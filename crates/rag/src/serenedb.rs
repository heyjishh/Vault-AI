//! Vector storage on [SereneDB](https://serenedb.com).
//!
//! SereneDB speaks the PostgreSQL wire protocol (v3.0, port 7890) and stores
//! embeddings in `FLOAT[N]` columns, searched with the `<=>` cosine-distance
//! operator. This implements the [`VectorStore`] trait over `tokio-postgres`,
//! so the rest of the pipeline is unchanged.
//!
//! ```sql
//! CREATE TABLE legal_chunks (
//!     chunk_id  TEXT PRIMARY KEY,
//!     doc_id    TEXT,
//!     ordinal   BIGINT,
//!     text      TEXT,
//!     metadata  TEXT,
//!     embedding FLOAT[1536]
//! );
//! CREATE INDEX legal_chunks_vec_idx ON legal_chunks
//!     USING inverted (embedding ivf (metric = 'cosine'));
//! ```

use async_trait::async_trait;
use legal_rag_common::Result;
use std::collections::HashMap;
use tracing::{info, warn};

use crate::vector_store::{ChunkRecord, ScoredChunk, VectorStore};

/// Escape a string for interpolation into a SQL literal.
fn esc(s: &str) -> String {
    s.replace('\'', "''")
}

/// Format an embedding as a SereneDB vector literal: `[0.1,0.2,...]`.
fn vector_literal(embedding: &[f32]) -> String {
    let inner: Vec<String> = embedding.iter().map(|v| format!("{v:?}")).collect();
    format!("[{}]", inner.join(","))
}

/// Parse the bracketed text form for a `FLOAT[N]` value:
/// `[0.1,0.2,...]` (with `[]` for an empty vector). This is the form
/// [`vector_literal`] writes; it is retained as a read fallback for any
/// server build that presents the returned column as text.
fn parse_vector_literal(value: &str) -> std::result::Result<Vec<f32>, String> {
    let inner = value
        .trim()
        .strip_prefix('[')
        .and_then(|v| v.strip_suffix(']'))
        .ok_or_else(|| format!("vector value is not bracketed: {value:?}"))?;
    if inner.trim().is_empty() {
        return Ok(Vec::new());
    }
    inner
        .split(',')
        .map(|part| {
            part.trim()
                .parse::<f32>()
                .map_err(|e| format!("invalid vector component {part:?}: {e}"))
        })
        .collect()
}

/// Maximum number of valid records whose delete + insert are grouped in
/// one upsert batch. Keeping each delete with its insert bounds the blast
/// radius of a mid-upsert database failure to one small batch.
const MAX_UPSERT_BATCH: usize = 25;

/// A record rejected before any SQL is issued because its embedding
/// cannot be stored in this store's `FLOAT[dims]` column.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SkippedUpsertRecord {
    id: String,
    embedding_len: usize,
    expected_dims: usize,
    reason: String,
}

/// The SQL-facing upsert plan: valid records grouped into bounded
/// batches, plus every rejected record. Skipped ids occur only in
/// `skipped` — never in a batch — so the delete that precedes each batch
/// insert can never remove an existing row for a skipped id.
#[derive(Debug)]
struct UpsertPlan {
    batches: Vec<Vec<ChunkRecord>>,
    skipped: Vec<SkippedUpsertRecord>,
}

/// Validate and batch an upsert without touching the database. A record
/// is valid only when its embedding length equals the store's configured
/// dimensions exactly; empty and wrong-sized embeddings are reported as
/// skipped rather than being allowed to fail (or precede deletion for)
/// the whole upsert.
fn plan_upsert(records: Vec<ChunkRecord>, expected_dims: usize) -> UpsertPlan {
    let mut valid = Vec::new();
    let mut skipped = Vec::new();
    for record in records {
        let embedding_len = record.embedding.len();
        if embedding_len == expected_dims {
            valid.push(record);
        } else {
            skipped.push(SkippedUpsertRecord {
                id: record.id.clone(),
                embedding_len,
                expected_dims,
                reason: format!(
                    "embedding has {embedding_len} dimension(s); expected {expected_dims}"
                ),
            });
        }
    }
    UpsertPlan {
        batches: valid.chunks(MAX_UPSERT_BATCH).map(|batch| batch.to_vec()).collect(),
        skipped,
    }
}

/// Build the CREATE TABLE statement (pure, unit-testable).
fn create_table_sql(table: &str, dims: usize) -> String {
    format!(
        "CREATE TABLE IF NOT EXISTS {table} (\
            chunk_id TEXT PRIMARY KEY, \
            doc_id TEXT, \
            ordinal BIGINT, \
            text TEXT, \
            metadata TEXT, \
            embedding FLOAT[{dims}])"
    )
}

/// Build the IVF vector index statement (pure, unit-testable).
fn create_index_sql(table: &str) -> String {
    format!(
        "CREATE INDEX IF NOT EXISTS {table}_vec_idx ON {table} \
         USING inverted (embedding ivf (metric = 'cosine'))"
    )
}

/// Vector store backed by SereneDB.
pub struct SereneDbVectorStore {
    client: tokio_postgres::Client,
    table: String,
    dims: usize,
}

impl SereneDbVectorStore {
    /// Connect and ensure the chunks table (+ IVF index) exists.
    pub async fn connect(url: &str, table: &str, dims: usize) -> Result<Self> {
        let (client, connection) =
            tokio_postgres::connect(url, tokio_postgres::NoTls)
                .await
                .map_err(|e| {
                    legal_rag_common::Error::Config(format!("serenedb connect failed: {e}"))
                })?;
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                warn!(error = %e, "serenedb connection closed");
            }
        });
        let store = Self { client, table: table.to_string(), dims };
        store.ensure_schema(dims).await?;
        info!(url = %mask_url(url), table = %table, "connected to serenedb");
        Ok(store)
    }

    async fn ensure_schema(&self, dims: usize) -> Result<()> {
        self.client
            .batch_execute(&create_table_sql(&self.table, dims))
            .await
            .map_err(|e| {
                legal_rag_common::Error::Config(format!("serenedb create table failed: {e}"))
            })?;
        // IVF index is an optimization; exact search works without it.
        if let Err(e) = self.client.batch_execute(&create_index_sql(&self.table)).await {
            warn!(error = %e, "serenedb vector index creation failed; continuing without it");
        }
        Ok(())
    }
}

/// Redact credentials from a URL for logging.
fn mask_url(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, rest)) => match rest.split_once('@') {
            Some((_, host)) => format!("{scheme}://***@{host}"),
            None => url.to_string(),
        },
        None => url.to_string(),
    }
}

#[async_trait]
impl VectorStore for SereneDbVectorStore {
    async fn upsert(&self, records: Vec<ChunkRecord>) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }

        // Validate the complete input BEFORE issuing any DELETE. Records
        // whose embeddings cannot fit this store's FLOAT[dims] column are
        // skipped and logged; they are excluded from every delete batch,
        // so an existing row with a skipped id remains untouched.
        let plan = plan_upsert(records, self.dims);
        if !plan.skipped.is_empty() {
            let skipped_ids: Vec<&str> = plan.skipped.iter().map(|s| s.id.as_str()).collect();
            warn!(
                table = %self.table,
                expected_dims = self.dims,
                skipped_count = plan.skipped.len(),
                skipped_ids = ?skipped_ids,
                skipped = ?plan.skipped,
                "SERENEDB UPSERT SKIPPED records with invalid embeddings; \
                 existing rows for skipped ids were left untouched"
            );
        }

        // Portable upsert in small batches: delete only this batch's
        // conflicting ids, then insert this batch before moving on.
        for batch in &plan.batches {
            let ids: Vec<String> =
                batch.iter().map(|r| format!("'{}'", esc(&r.id))).collect();
            self.client
                .batch_execute(&format!(
                    "DELETE FROM {} WHERE chunk_id IN ({})",
                    self.table,
                    ids.join(",")
                ))
                .await
                .map_err(|e| {
                    legal_rag_common::Error::BadRequest(format!(
                        "serenedb delete failed: {e}"
                    ))
                })?;

            let rows: Vec<String> = batch
                .iter()
                .map(|r| {
                    let meta = serde_json::to_string(&r.metadata).unwrap_or_default();
                    format!(
                        "('{}','{}',{},'{}','{}','{}')",
                        esc(&r.id),
                        esc(&r.doc_id),
                        r.ordinal,
                        esc(&r.text),
                        esc(&meta),
                        vector_literal(&r.embedding),
                    )
                })
                .collect();
            let sql = format!(
                "INSERT INTO {} (chunk_id, doc_id, ordinal, text, metadata, embedding) VALUES {}",
                self.table,
                rows.join(",")
            );
            self.client.batch_execute(&sql).await.map_err(|e| {
                legal_rag_common::Error::BadRequest(format!("serenedb insert failed: {e}"))
            })?;
        }
        Ok(())
    }

    async fn search(&self, query: &[f32], top_k: usize) -> Result<Vec<ScoredChunk>> {
        // The embedding is consumed in-database to compute distance; hit
        // records intentionally do not carry a copy back to callers.
        let sql = format!(
            "SELECT chunk_id, doc_id, ordinal, text, metadata, \
             (embedding <=> '{}')::float8 AS distance \
             FROM {} ORDER BY distance ASC LIMIT {}",
            vector_literal(query),
            self.table,
            top_k.max(1),
        );
        let rows = self.client.query(&sql, &[]).await.map_err(|e| {
            legal_rag_common::Error::BadRequest(format!("serenedb search failed: {e}"))
        })?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let distance: f64 = row.try_get("distance").map_err(|e| {
                legal_rag_common::Error::BadRequest(format!("serenedb row decode failed: {e}"))
            })?;
            // Cosine distance in [0,2] → similarity score in (0,1], monotonic.
            let score = (1.0 / (1.0 + distance.max(0.0))) as f32;
            let metadata: HashMap<String, String> = row
                .try_get::<_, String>("metadata")
                .ok()
                .and_then(|m| serde_json::from_str(&m).ok())
                .unwrap_or_default();
            out.push(ScoredChunk {
                score,
                record: ChunkRecord {
                    id: row.try_get("chunk_id").map_err(|e| {
                        legal_rag_common::Error::BadRequest(format!("row decode: {e}"))
                    })?,
                    doc_id: row.try_get("doc_id").map_err(|e| {
                        legal_rag_common::Error::BadRequest(format!("row decode: {e}"))
                    })?,
                    ordinal: row
                        .try_get::<_, i64>("ordinal")
                        .map(|o| o as usize)
                        .unwrap_or(0),
                    text: row.try_get("text").map_err(|e| {
                        legal_rag_common::Error::BadRequest(format!("row decode: {e}"))
                    })?,
                    embedding: Vec::new(),
                    metadata,
                },
            });
        }
        Ok(out)
    }

    async fn get_by_ids(&self, ids: &[String]) -> Result<Vec<ChunkRecord>> {
        // Text/metadata projection: callers fetch records already chosen
        // by vector or keyword retrieval and do not consume embeddings.
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let list: Vec<String> = ids.iter().map(|id| format!("'{}'", esc(id))).collect();
        let sql = format!(
            "SELECT chunk_id, doc_id, ordinal, text, metadata FROM {} WHERE chunk_id IN ({})",
            self.table,
            list.join(",")
        );
        let rows = self.client.query(&sql, &[]).await.map_err(|e| {
            legal_rag_common::Error::BadRequest(format!("serenedb get_by_ids failed: {e}"))
        })?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let metadata: HashMap<String, String> = row
                .try_get::<_, String>("metadata")
                .ok()
                .and_then(|m| serde_json::from_str(&m).ok())
                .unwrap_or_default();
            out.push(ChunkRecord {
                id: row.try_get("chunk_id").map_err(|e| {
                    legal_rag_common::Error::BadRequest(format!("row decode: {e}"))
                })?,
                doc_id: row.try_get("doc_id").map_err(|e| {
                    legal_rag_common::Error::BadRequest(format!("row decode: {e}"))
                })?,
                ordinal: row.try_get::<_, i64>("ordinal").map(|o| o as usize).unwrap_or(0),
                text: row.try_get("text").map_err(|e| {
                    legal_rag_common::Error::BadRequest(format!("row decode: {e}"))
                })?,
                embedding: Vec::new(),
                metadata,
            });
        }
        Ok(out)
    }

    async fn len(&self) -> usize {
        let sql = format!("SELECT COUNT(*) AS n FROM {}", self.table);
        self.client
            .query_one(&sql, &[])
            .await
            .ok()
            .and_then(|row| row.try_get::<_, i64>("n").ok())
            .map(|n| n.max(0) as usize)
            .unwrap_or(0)
    }

    async fn remove_doc(&self, doc_id: &str) -> Result<usize> {
        let sql = format!("DELETE FROM {} WHERE doc_id = '{}'", self.table, esc(doc_id));
        let n = self.client.execute(&sql, &[]).await.map_err(|e| {
            legal_rag_common::Error::BadRequest(format!("serenedb delete failed: {e}"))
        })?;
        Ok(n as usize)
    }

    async fn chunks_of_doc(&self, doc_id: &str) -> Result<Vec<ChunkRecord>> {
        // Text-only projection by design: callers use this for document
        // text and chunk-id presence, never for persistence.
        let sql = format!(
            "SELECT chunk_id, doc_id, ordinal, text, metadata FROM {} WHERE doc_id = '{}' ORDER BY ordinal ASC",
            self.table,
            esc(doc_id)
        );
        let rows = self.client.query(&sql, &[]).await.map_err(|e| {
            legal_rag_common::Error::BadRequest(format!("serenedb chunks_of_doc failed: {e}"))
        })?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let metadata: HashMap<String, String> = row
                .try_get::<_, String>("metadata")
                .ok()
                .and_then(|m| serde_json::from_str(&m).ok())
                .unwrap_or_default();
            out.push(ChunkRecord {
                id: row.try_get("chunk_id").map_err(|e| {
                    legal_rag_common::Error::BadRequest(format!("row decode: {e}"))
                })?,
                doc_id: row.try_get("doc_id").map_err(|e| {
                    legal_rag_common::Error::BadRequest(format!("row decode: {e}"))
                })?,
                ordinal: row.try_get::<_, i64>("ordinal").map(|o| o as usize).unwrap_or(0),
                text: row.try_get("text").map_err(|e| {
                    legal_rag_common::Error::BadRequest(format!("row decode: {e}"))
                })?,
                embedding: Vec::new(),
                metadata,
            });
        }
        Ok(out)
    }

    async fn chunk_ids(&self) -> Result<Vec<String>> {
        let sql = format!("SELECT chunk_id FROM {}", self.table);
        let rows = self.client.query(&sql, &[]).await.map_err(|e| {
            legal_rag_common::Error::BadRequest(format!("serenedb chunk_ids failed: {e}"))
        })?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(row.try_get("chunk_id").map_err(|e| {
                legal_rag_common::Error::BadRequest(format!("row decode: {e}"))
            })?);
        }
        Ok(out)
    }

    async fn all_chunks(&self) -> Result<Vec<ChunkRecord>> {
        // Full backup projection, unlike the text-only projections above:
        // the server snapshot saver dumps through all_chunks, so every
        // record must carry its embedding or a later restore cannot write
        // it back to the FLOAT[dims] column.
        let sql = format!(
            "SELECT chunk_id, doc_id, ordinal, text, metadata, embedding \
             FROM {} ORDER BY doc_id ASC, ordinal ASC",
            self.table
        );
        let rows = self.client.query(&sql, &[]).await.map_err(|e| {
            legal_rag_common::Error::BadRequest(format!("serenedb all_chunks failed: {e}"))
        })?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let metadata: HashMap<String, String> = row
                .try_get::<_, String>("metadata")
                .ok()
                .and_then(|m| serde_json::from_str(&m).ok())
                .unwrap_or_default();
            // SereneDB writes vectors from the bracketed text literal,
            // but tokio-postgres exposes the returned FLOAT[N] value as
            // an f32 array. Accept that typed form first and the text
            // form as a fallback if a server build presents it as text.
            let embedding = match row.try_get::<_, Vec<f32>>("embedding") {
                Ok(embedding) => embedding,
                Err(array_err) => {
                    let embedding_text: String =
                        row.try_get("embedding").map_err(|text_err| {
                            legal_rag_common::Error::BadRequest(format!(
                                "serenedb embedding decode failed: array decode: {array_err}; \
                                 text decode: {text_err}"
                            ))
                        })?;
                    parse_vector_literal(&embedding_text).map_err(|e| {
                        legal_rag_common::Error::BadRequest(format!(
                            "serenedb embedding decode failed: {e}"
                        ))
                    })?
                }
            };
            out.push(ChunkRecord {
                id: row.try_get("chunk_id").map_err(|e| {
                    legal_rag_common::Error::BadRequest(format!("row decode: {e}"))
                })?,
                doc_id: row.try_get("doc_id").map_err(|e| {
                    legal_rag_common::Error::BadRequest(format!("row decode: {e}"))
                })?,
                ordinal: row.try_get::<_, i64>("ordinal").map(|o| o as usize).unwrap_or(0),
                text: row.try_get("text").map_err(|e| {
                    legal_rag_common::Error::BadRequest(format!("row decode: {e}"))
                })?,
                embedding,
                metadata,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ddl_matches_documented_syntax() {
        let ddl = create_table_sql("legal_chunks", 1536);
        assert!(ddl.contains("embedding FLOAT[1536]"));
        assert!(ddl.contains("chunk_id TEXT PRIMARY KEY"));
        let idx = create_index_sql("legal_chunks");
        assert!(idx.contains("USING inverted"));
        assert!(idx.contains("ivf (metric = 'cosine')"));
    }

    #[test]
    fn vector_literal_format() {
        assert_eq!(vector_literal(&[0.5, -1.25]), "[0.5,-1.25]");
    }

    #[test]
    fn sql_escaping() {
        assert_eq!(esc("o'clock"), "o''clock");
    }

    fn plan_record(id: &str, embedding: Vec<f32>) -> ChunkRecord {
        ChunkRecord {
            id: id.into(),
            doc_id: "doc-plan".into(),
            ordinal: 0,
            text: id.into(),
            embedding,
            metadata: HashMap::new(),
        }
    }

    #[test]
    fn parse_vector_literal_reads_serenedb_text_form() {
        assert_eq!(
            parse_vector_literal("[0.5, -1.25, 1e-3]").unwrap(),
            vec![0.5, -1.25, 0.001]
        );
        assert_eq!(parse_vector_literal("[]").unwrap(), Vec::<f32>::new());
        assert!(parse_vector_literal("0.5,1.0").is_err());
        assert!(parse_vector_literal("[0.5,nope]").is_err());
    }

    #[test]
    fn plan_upsert_partitions_invalid_records_and_never_batches_skipped_ids() {
        let mut records = vec![
            plan_record("existing-but-invalid", Vec::new()),
            plan_record("wrong-dimensions", vec![1.0, 0.0]),
        ];
        records.extend(
            (0..30).map(|i| plan_record(&format!("valid-{i}"), vec![0.0, 1.0, 0.0])),
        );
        records.push(plan_record("late-invalid", vec![1.0]));

        let plan = plan_upsert(records, 3);
        let batch_sizes: Vec<usize> = plan.batches.iter().map(|batch| batch.len()).collect();
        assert_eq!(batch_sizes, vec![MAX_UPSERT_BATCH, 5]);

        let batched_ids: Vec<&str> = plan
            .batches
            .iter()
            .flat_map(|batch| batch.iter().map(|record| record.id.as_str()))
            .collect();
        assert_eq!(batched_ids.len(), 30);
        assert!(batched_ids.contains(&"valid-0"));
        assert!(batched_ids.contains(&"valid-29"));
        for skipped in &plan.skipped {
            assert!(
                !batched_ids.contains(&skipped.id.as_str()),
                "skipped id {} must never enter a delete/insert batch",
                skipped.id
            );
        }
        assert_eq!(
            plan.skipped,
            vec![
                SkippedUpsertRecord {
                    id: "existing-but-invalid".into(),
                    embedding_len: 0,
                    expected_dims: 3,
                    reason: "embedding has 0 dimension(s); expected 3".into(),
                },
                SkippedUpsertRecord {
                    id: "wrong-dimensions".into(),
                    embedding_len: 2,
                    expected_dims: 3,
                    reason: "embedding has 2 dimension(s); expected 3".into(),
                },
                SkippedUpsertRecord {
                    id: "late-invalid".into(),
                    embedding_len: 1,
                    expected_dims: 3,
                    reason: "embedding has 1 dimension(s); expected 3".into(),
                },
            ]
        );
    }

    #[test]
    fn plan_upsert_batches_valid_records_at_twenty_five() {
        let records = (0..26)
            .map(|i| plan_record(&format!("valid-{i}"), vec![1.0, 0.0]))
            .collect();
        let plan = plan_upsert(records, 2);
        let batch_sizes: Vec<usize> = plan.batches.iter().map(|batch| batch.len()).collect();
        assert_eq!(batch_sizes, vec![25, 1]);
        assert!(plan.skipped.is_empty());

        let only_invalid = plan_upsert(vec![plan_record("empty", Vec::new())], 2);
        assert!(only_invalid.batches.is_empty());
        assert_eq!(only_invalid.skipped.len(), 1);
    }

    fn live_record(
        id: &str,
        doc_id: &str,
        text: &str,
        embedding: Vec<f32>,
    ) -> ChunkRecord {
        ChunkRecord {
            id: id.into(),
            doc_id: doc_id.into(),
            ordinal: 0,
            text: text.into(),
            embedding,
            metadata: HashMap::new(),
        }
    }

    async fn exercise_live_upsert_safety(
        store: &SereneDbVectorStore,
    ) -> std::result::Result<(), String> {
        store
            .upsert(vec![
                live_record(
                    "existing-invalid-target",
                    "seed-doc",
                    "original row must survive",
                    vec![1.0, 0.0, 0.0, 0.0],
                ),
                live_record(
                    "existing-keep",
                    "seed-doc",
                    "unrelated seeded row",
                    vec![0.0, 1.0, 0.0, 0.0],
                ),
            ])
            .await
            .map_err(|e| format!("seed upsert failed: {e}"))?;

        let mut second = vec![
            live_record(
                "existing-invalid-target",
                "replacement-doc",
                "this replacement must not land",
                Vec::new(),
            ),
            live_record(
                "invalid-wrong-dimensions",
                "invalid-doc",
                "this record must not land",
                vec![1.0, 0.0],
            ),
        ];
        second.extend((0..26).map(|i| {
            live_record(
                &format!("new-valid-{i}"),
                "new-doc",
                "new valid row",
                vec![0.0, 0.0, 1.0, 0.0],
            )
        }));
        store
            .upsert(second)
            .await
            .map_err(|e| format!("mixed upsert failed: {e}"))?;

        let all = store
            .all_chunks()
            .await
            .map_err(|e| format!("all_chunks failed: {e}"))?;
        let by_id: HashMap<String, ChunkRecord> =
            all.into_iter().map(|record| (record.id.clone(), record)).collect();
        if by_id.len() != 28 {
            return Err(format!("expected 28 rows after safe upsert, got {}", by_id.len()));
        }
        let existing = by_id
            .get("existing-invalid-target")
            .ok_or("seeded row targeted by an invalid record disappeared")?;
        if existing.text != "original row must survive"
            || existing.embedding != vec![1.0, 0.0, 0.0, 0.0]
        {
            return Err(format!(
                "seeded row targeted by an invalid record changed: {existing:?}"
            ));
        }
        if !by_id.contains_key("existing-keep") {
            return Err("unrelated seeded row disappeared".into());
        }
        if by_id.contains_key("invalid-wrong-dimensions") {
            return Err("wrong-dimension record was inserted".into());
        }
        for i in 0..26 {
            let id = format!("new-valid-{i}");
            let record = by_id
                .get(&id)
                .ok_or_else(|| format!("valid new row {id} did not land"))?;
            if record.embedding != vec![0.0, 0.0, 1.0, 0.0] {
                return Err(format!(
                    "all_chunks returned the wrong embedding for {id}: {:?}",
                    record.embedding
                ));
            }
        }
        Ok(())
    }

    /// Live upsert-safety test — runs only when SERENEDB_URL is set.
    /// Uses a unique scratch table and drops it after the assertions.
    #[tokio::test]
    async fn live_serenedb_upsert_skips_invalid_without_deleting_existing_rows() {
        let Some(url) = std::env::var("SERENEDB_URL").ok() else {
            eprintln!("skipping: SERENEDB_URL not set");
            return;
        };
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let table = format!("legal_chunks_p21_{}_{}", std::process::id(), suffix);
        let store = SereneDbVectorStore::connect(&url, &table, 4).await.unwrap();

        let outcome = exercise_live_upsert_safety(&store).await;
        let cleanup = store
            .client
            .batch_execute(&format!("DROP TABLE IF EXISTS {table}"))
            .await;
        assert!(cleanup.is_ok(), "scratch table cleanup failed: {:?}", cleanup.err());
        assert!(outcome.is_ok(), "live upsert safety check failed: {:?}", outcome.err());
    }

    /// Live integration test — runs only when SERENEDB_URL is set.
    #[tokio::test]
    async fn live_serenedb_roundtrip() {
        let Some(url) = std::env::var("SERENEDB_URL").ok() else {
            eprintln!("skipping: SERENEDB_URL not set");
            return;
        };
        let store = SereneDbVectorStore::connect(&url, "legal_chunks_test", 4).await.unwrap();
        let rec = ChunkRecord {
            id: "test-chunk-1".into(),
            doc_id: "test-doc".into(),
            ordinal: 0,
            text: "hello world".into(),
            embedding: vec![1.0, 0.0, 0.0, 0.0],
            metadata: HashMap::new(),
        };
        store.upsert(vec![rec]).await.unwrap();
        let hits = store.search(&[1.0, 0.0, 0.0, 0.0], 5).await.unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0].record.id, "test-chunk-1");
        assert!(store.len().await >= 1);
        let got = store.get_by_ids(&["test-chunk-1".to_string()]).await.unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(store.remove_doc("test-doc").await.unwrap(), 1);
        assert_eq!(store.len().await, 0);
    }
}
