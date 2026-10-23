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
        let store = Self { client, table: table.to_string() };
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
        // Portable upsert: delete conflicting ids, then multi-row insert.
        let ids: Vec<String> =
            records.iter().map(|r| format!("'{}'", esc(&r.id))).collect();
        self.client
            .batch_execute(&format!(
                "DELETE FROM {} WHERE chunk_id IN ({})",
                self.table,
                ids.join(",")
            ))
            .await
            .map_err(|e| {
                legal_rag_common::Error::BadRequest(format!("serenedb delete failed: {e}"))
            })?;

        for batch in records.chunks(200) {
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
        // Same projection as chunks_of_doc (no embedding column), unfiltered.
        let sql = format!(
            "SELECT chunk_id, doc_id, ordinal, text, metadata FROM {} ORDER BY doc_id ASC, ordinal ASC",
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
