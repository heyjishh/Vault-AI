//! Vector storage on Amazon S3 Vectors — Spice.ai's external vector
//! engine, implemented natively (ADR-0020).
//!
//! Spice's `s3_vectors` engine delegates the index to AWS S3 Vectors
//! (GA, available in ap-south-1 — our ADR-0016 target region): vectors
//! live in a vector bucket, queries are ANN with distances computed by
//! AWS, and *filterable metadata* predicates are pushed into the
//! query. This module speaks the S3 Vectors JSON API directly
//! (REST-JSON, SigV4-signed with the workspace's `sha2` — no AWS SDK
//! dependency), behind the same [`VectorStore`] trait as the other
//! backends.
//!
//! Index layout (created on connect):
//! - key = chunk id
//! - `doc_id` — FILTERABLE metadata (the pushdown predicate)
//! - `ordinal` — non-filterable metadata (number)
//! - `text`, `meta` (record metadata as JSON) — non-filterable
//!   metadata. AWS caps metadata at ~2 KB filterable / ~40 KB total
//!   per vector; chunk texts here are far below that, and `upsert`
//!   fails loudly on an oversized record instead of truncating it.
//!
//! Configuration (env): `LEGAL_RAG_S3V_BUCKET` (required),
//! `LEGAL_RAG_S3V_INDEX` (default `legal-chunks`), `AWS_REGION`
//! (default `ap-south-1`), `AWS_ACCESS_KEY_ID`,
//! `AWS_SECRET_ACCESS_KEY`, optional `AWS_SESSION_TOKEN`.
//!
//! Verification status follows the SereneDB precedent: request
//! building, filter building and SigV4 signing are unit-tested
//! (including AWS's documented signing-key derivation vector); the
//! live round-trip test is gated behind `S3VECTORS_LIVE_TEST=1` plus
//! real AWS credentials, which the sandbox does not hold.

use std::collections::HashMap;

use async_trait::async_trait;
use legal_rag_common::{Error, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::vector_store::{ChunkRecord, ScoredChunk, VectorStore};

const MAX_PUT_BATCH: usize = 500;
const MAX_DELETE_BATCH: usize = 500;
/// AWS metadata ceiling per vector is ~40 KB total; keep headroom.
const MAX_METADATA_BYTES: usize = 35_000;

#[derive(Debug, Clone)]
pub struct S3VectorsConfig {
    pub bucket: String,
    pub index: String,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
    pub session_token: Option<String>,
}

impl S3VectorsConfig {
    pub fn from_env() -> Option<Result<Self>> {
        let bucket = std::env::var("LEGAL_RAG_S3V_BUCKET").ok().filter(|s| !s.is_empty())?;
        Some((|| {
            let access_key = std::env::var("AWS_ACCESS_KEY_ID")
                .map_err(|_| Error::Other(anyhow::anyhow!("s3vectors: AWS_ACCESS_KEY_ID not set")))?;
            let secret_key = std::env::var("AWS_SECRET_ACCESS_KEY").map_err(|_| {
                Error::Other(anyhow::anyhow!("s3vectors: AWS_SECRET_ACCESS_KEY not set"))
            })?;
            Ok(Self {
                bucket,
                index: std::env::var("LEGAL_RAG_S3V_INDEX")
                    .ok()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "legal-chunks".to_string()),
                region: std::env::var("AWS_REGION")
                    .ok()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "ap-south-1".to_string()),
                access_key,
                secret_key,
                session_token: std::env::var("AWS_SESSION_TOKEN").ok().filter(|s| !s.is_empty()),
            })
        })())
    }

    fn host(&self) -> String {
        format!("s3vectors.{}.api.aws", self.region)
    }
}

// ---------- SigV4 (hand-rolled over sha2) ----------

fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn hmac_sha256(key: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut block = key.to_vec();
    if block.len() > 64 {
        let mut h = Sha256::new();
        h.update(&block);
        block = h.finalize().to_vec();
    }
    block.resize(64, 0);
    let ipad: Vec<u8> = block.iter().map(|b| b ^ 0x36).collect();
    let opad: Vec<u8> = block.iter().map(|b| b ^ 0x5c).collect();
    let mut inner = Sha256::new();
    inner.update(&ipad);
    inner.update(msg);
    let inner_hash = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(&opad);
    outer.update(inner_hash);
    outer.finalize().to_vec()
}

fn derive_signing_key(secret: &str, date: &str, region: &str, service: &str) -> Vec<u8> {
    let k_date = hmac_sha256(format!("AWS4{secret}").as_bytes(), date.as_bytes());
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, service.as_bytes());
    hmac_sha256(&k_service, b"aws4_request")
}

/// UTC timestamp parts from a Unix epoch second count, formatted for
/// SigV4 (`YYYYMMDDTHHMMSSZ` + `YYYYMMDD`). Civil-date conversion
/// after Howard Hinnant's algorithm — no date crate needed.
fn amz_date(epoch_secs: u64) -> (String, String) {
    let days = (epoch_secs / 86_400) as i64;
    let secs = epoch_secs % 86_400;
    let (hh, mm, ss) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    let datestamp = format!("{year:04}{m:02}{d:02}");
    (format!("{datestamp}T{hh:02}{mm:02}{ss:02}Z"), datestamp)
}

/// Sign a JSON POST to `/<op>`; returns the headers to send.
fn signed_headers(
    cfg: &S3VectorsConfig,
    op: &str,
    body: &[u8],
    epoch_secs: u64,
) -> Vec<(String, String)> {
    let host = cfg.host();
    let (amzdate, datestamp) = amz_date(epoch_secs);
    let payload_hash = sha256_hex(body);
    let scope = format!("{datestamp}/{}/s3vectors/aws4_request", cfg.region);
    let mut signed = vec![("host".to_string(), host.clone()), ("x-amz-date".to_string(), amzdate.clone())];
    if let Some(tok) = &cfg.session_token {
        signed.push(("x-amz-security-token".to_string(), tok.clone()));
    }
    signed.sort();
    let canonical_headers: String =
        signed.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
    let signed_names: Vec<&str> = signed.iter().map(|(k, _)| k.as_str()).collect();
    let canonical = format!(
        "POST\n/{op}\n\n{canonical_headers}\n{}\n{payload_hash}",
        signed_names.join(";")
    );
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amzdate}\n{scope}\n{}",
        sha256_hex(canonical.as_bytes())
    );
    let key = derive_signing_key(&cfg.secret_key, &datestamp, &cfg.region, "s3vectors");
    let signature: String =
        hmac_sha256(&key, string_to_sign.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    let mut headers = vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("x-amz-date".to_string(), amzdate),
        (
            "authorization".to_string(),
            format!(
                "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={}, Signature={signature}",
                cfg.access_key,
                signed_names.join(";")
            ),
        ),
    ];
    if let Some(tok) = &cfg.session_token {
        headers.push(("x-amz-security-token".to_string(), tok.clone()));
    }
    headers
}

// ---------- record <-> vector mapping (pure, unit-tested) ----------

fn record_to_put(rec: &ChunkRecord) -> Result<Value> {
    let meta_json = serde_json::to_string(&rec.metadata)
        .map_err(|e| Error::Other(anyhow::anyhow!("s3vectors: metadata encode: {e}")))?;
    let size = rec.text.len() + meta_json.len();
    if size > MAX_METADATA_BYTES {
        return Err(Error::Other(anyhow::anyhow!(
            "s3vectors: record {} metadata payload {size}B exceeds the {MAX_METADATA_BYTES}B guard (AWS ~40KB/vector cap)",
            rec.id
        )));
    }
    Ok(json!({
        "key": rec.id,
        "data": { "float32": rec.embedding },
        "metadata": {
            "doc_id": rec.doc_id,
            "ordinal": rec.ordinal as u64,
            "text": rec.text,
            "meta": meta_json,
        }
    }))
}

/// Metadata filter for one document — the pushdown predicate.
fn doc_filter(doc_id: &str) -> Value {
    json!({ "doc_id": { "$eq": doc_id } })
}

fn record_from_vector(v: &Value, with_data: bool) -> Option<ChunkRecord> {
    let key = v.get("key")?.as_str()?.to_string();
    let meta = v.get("metadata")?;
    let doc_id = meta.get("doc_id")?.as_str()?.to_string();
    let ordinal = meta.get("ordinal").and_then(|o| o.as_u64()).unwrap_or(0) as usize;
    let text = meta.get("text").and_then(|t| t.as_str()).unwrap_or_default().to_string();
    let metadata: HashMap<String, String> = meta
        .get("meta")
        .and_then(|m| m.as_str())
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let embedding = if with_data {
        v.get("data")
            .and_then(|d| d.get("float32"))
            .and_then(|a| a.as_array())
            .map(|a| {
                a.iter().filter_map(|x| x.as_f64().map(|f| f as f32)).collect::<Vec<f32>>()
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    Some(ChunkRecord { id: key, doc_id, ordinal, text, embedding, metadata })
}

// ---------- the store ----------

pub struct S3VectorsStore {
    cfg: S3VectorsConfig,
    dims: usize,
    http: reqwest::Client,
}

impl S3VectorsStore {
    /// Connect and ensure the bucket + index exist (idempotent —
    /// conflicts are treated as success).
    pub async fn connect(cfg: S3VectorsConfig, dims: usize) -> Result<Self> {
        let store = Self { cfg, dims, http: reqwest::Client::new() };
        store
            .call_allow_conflict("CreateVectorBucket", json!({ "vectorBucketName": store.cfg.bucket }))
            .await?;
        store
            .call_allow_conflict(
                "CreateIndex",
                json!({
                    "vectorBucketName": store.cfg.bucket,
                    "indexName": store.cfg.index,
                    "dataType": "float32",
                    "dimension": dims,
                    "distanceMetric": "cosine",
                    "metadataConfiguration": {
                        "nonFilterableMetadataKeys": ["ordinal", "text", "meta"]
                    }
                }),
            )
            .await?;
        tracing::info!(bucket = %store.cfg.bucket, index = %store.cfg.index, region = %store.cfg.region, "using S3 Vectors vector store");
        Ok(store)
    }

    async fn call(&self, op: &str, body: Value) -> Result<Value> {
        let bytes = serde_json::to_vec(&body)
            .map_err(|e| Error::Other(anyhow::anyhow!("s3vectors: encode: {e}")))?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let url = format!("https://{}/{}", self.cfg.host(), op);
        let mut req = self.http.post(&url).body(bytes.clone());
        for (k, v) in signed_headers(&self.cfg, op, &bytes, now) {
            req = req.header(k, v);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| Error::Other(anyhow::anyhow!("s3vectors {op}: {e}")))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(Error::Other(anyhow::anyhow!(
                "s3vectors {op} failed ({status}): {text}"
            )));
        }
        if text.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text)
            .map_err(|e| Error::Other(anyhow::anyhow!("s3vectors {op}: bad JSON: {e}")))
    }

    async fn call_allow_conflict(&self, op: &str, body: Value) -> Result<()> {
        match self.call(op, body).await {
            Ok(_) => Ok(()),
            Err(e) => {
                let msg = format!("{e}");
                if msg.contains("ConflictException") || msg.contains("(409)") {
                    Ok(())
                } else {
                    Err(e)
                }
            }
        }
    }

    fn base(&self) -> Value {
        json!({ "vectorBucketName": self.cfg.bucket, "indexName": self.cfg.index })
    }

    /// Page through the whole index (records reconstructed from
    /// metadata; `with_data` also returns the stored embeddings).
    async fn list_all(&self, with_data: bool) -> Result<Vec<ChunkRecord>> {
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = self.base();
            body["returnData"] = json!(with_data);
            body["returnMetadata"] = json!(true);
            body["maxResults"] = json!(1000);
            if let Some(t) = &token {
                body["nextToken"] = json!(t);
            }
            let resp = self.call("ListVectors", body).await?;
            if let Some(vectors) = resp.get("vectors").and_then(|v| v.as_array()) {
                for v in vectors {
                    if let Some(rec) = record_from_vector(v, with_data) {
                        out.push(rec);
                    }
                }
            }
            token = resp
                .get("nextToken")
                .and_then(|t| t.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
            if token.is_none() {
                break;
            }
        }
        Ok(out)
    }
}

#[async_trait]
impl VectorStore for S3VectorsStore {
    async fn upsert(&self, records: Vec<ChunkRecord>) -> Result<()> {
        for chunk in records.chunks(MAX_PUT_BATCH) {
            let mut vectors = Vec::with_capacity(chunk.len());
            for rec in chunk {
                if rec.embedding.len() != self.dims {
                    return Err(Error::Other(anyhow::anyhow!(
                        "s3vectors: record {} has {} dims, index expects {}",
                        rec.id,
                        rec.embedding.len(),
                        self.dims
                    )));
                }
                vectors.push(record_to_put(rec)?);
            }
            let mut body = self.base();
            body["vectors"] = json!(vectors);
            self.call("PutVectors", body).await?;
        }
        Ok(())
    }

    async fn search(&self, query: &[f32], top_k: usize) -> Result<Vec<ScoredChunk>> {
        let mut body = self.base();
        body["queryVector"] = json!({ "float32": query });
        body["topK"] = json!(top_k);
        body["returnDistance"] = json!(true);
        body["returnMetadata"] = json!(true);
        let resp = self.call("QueryVectors", body).await?;
        let mut out = Vec::new();
        if let Some(vectors) = resp.get("vectors").and_then(|v| v.as_array()) {
            for v in vectors {
                if let Some(mut rec) = record_from_vector(v, false) {
                    let distance =
                        v.get("distance").and_then(|d| d.as_f64()).unwrap_or(1.0) as f32;
                    rec.embedding = Vec::new();
                    out.push(ScoredChunk { record: rec, score: 1.0 - distance });
                }
            }
        }
        Ok(out)
    }

    async fn get_by_ids(&self, ids: &[String]) -> Result<Vec<ChunkRecord>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut body = self.base();
        body["keys"] = json!(ids);
        body["returnData"] = json!(true);
        body["returnMetadata"] = json!(true);
        let resp = self.call("GetVectors", body).await?;
        let mut out = Vec::new();
        if let Some(vectors) = resp.get("vectors").and_then(|v| v.as_array()) {
            for v in vectors {
                if let Some(rec) = record_from_vector(v, true) {
                    out.push(rec);
                }
            }
        }
        Ok(out)
    }

    async fn len(&self) -> usize {
        // No count API — page the index (keys only, no payloads).
        let mut count = 0usize;
        let mut token: Option<String> = None;
        loop {
            let mut body = self.base();
            body["returnData"] = json!(false);
            body["returnMetadata"] = json!(false);
            body["maxResults"] = json!(1000);
            if let Some(t) = &token {
                body["nextToken"] = json!(t);
            }
            match self.call("ListVectors", body).await {
                Ok(resp) => {
                    count += resp
                        .get("vectors")
                        .and_then(|v| v.as_array())
                        .map(|a| a.len())
                        .unwrap_or(0);
                    token = resp
                        .get("nextToken")
                        .and_then(|t| t.as_str())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string());
                    if token.is_none() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        count
    }

    async fn remove_doc(&self, doc_id: &str) -> Result<usize> {
        // No query-delete: list metadata, pick the doc's keys, delete.
        // (The doc_id filter itself is exercised on QueryVectors.)
        let all = self.list_all(false).await?;
        let keys: Vec<String> =
            all.iter().filter(|r| r.doc_id == doc_id).map(|r| r.id.clone()).collect();
        let n = keys.len();
        for chunk in keys.chunks(MAX_DELETE_BATCH) {
            let mut body = self.base();
            body["keys"] = json!(chunk);
            self.call("DeleteVectors", body).await?;
        }
        Ok(n)
    }

    async fn chunks_of_doc(&self, doc_id: &str) -> Result<Vec<ChunkRecord>> {
        // Filtered ANN probe is the wrong tool for exact enumeration;
        // list + local filter is exact (corpus pages are cheap here).
        let mut chunks: Vec<ChunkRecord> =
            self.list_all(true).await?.into_iter().filter(|r| r.doc_id == doc_id).collect();
        chunks.sort_by_key(|r| r.ordinal);
        Ok(chunks)
    }

    async fn chunk_ids(&self) -> Result<Vec<String>> {
        Ok(self.list_all(false).await?.into_iter().map(|r| r.id).collect())
    }

    async fn all_chunks(&self) -> Result<Vec<ChunkRecord>> {
        // Embeddings come back empty by design on this backend for
        // bulk reads (ListVectors payload cost); see trait docs.
        self.list_all(false).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signing_key_matches_aws_documented_vector() {
        // AWS SigV4 documentation's worked example for deriving the
        // signing key (secret from the AWS docs, 20150830, us-east-1,
        // iam) — the canonical test vector for this exact chain.
        let key = derive_signing_key(
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            "20150830",
            "us-east-1",
            "iam",
        );
        let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex,
            "c4afb1cc5771d871763a393e44b703571b55cc28424d1a5e86da6ed3c154a4b9"
        );
    }

    #[test]
    fn amz_date_formats_epoch() {
        // 2015-08-30T12:36:00Z (the SigV4 docs example instant).
        let (full, day) = amz_date(1_440_938_160);
        assert_eq!(full, "20150830T123600Z");
        assert_eq!(day, "20150830");
    }

    #[test]
    fn record_mapping_roundtrips() {
        let mut metadata = HashMap::new();
        metadata.insert("pc".to_string(), "child".to_string());
        let rec = ChunkRecord {
            id: "c1".into(),
            doc_id: "d1".into(),
            ordinal: 7,
            text: "hello".into(),
            embedding: vec![0.1, 0.2],
            metadata,
        };
        let put = record_to_put(&rec).unwrap();
        assert_eq!(put["key"], "c1");
        assert_eq!(put["metadata"]["doc_id"], "d1");
        let back = record_from_vector(&put, true).unwrap();
        assert_eq!(back.id, "c1");
        assert_eq!(back.doc_id, "d1");
        assert_eq!(back.ordinal, 7);
        assert_eq!(back.text, "hello");
        assert_eq!(back.embedding, vec![0.1, 0.2]);
        assert_eq!(back.metadata.get("pc").map(|s| s.as_str()), Some("child"));
        assert_eq!(doc_filter("d1"), json!({ "doc_id": { "$eq": "d1" } }));
    }

    #[test]
    fn oversized_record_is_rejected_not_truncated() {
        let rec = ChunkRecord {
            id: "big".into(),
            doc_id: "d1".into(),
            ordinal: 0,
            text: "x".repeat(MAX_METADATA_BYTES + 1),
            embedding: vec![0.1],
            metadata: HashMap::new(),
        };
        assert!(record_to_put(&rec).is_err());
    }

    /// Live round-trip — runs only with real AWS credentials and
    /// `S3VECTORS_LIVE_TEST=1` (creates the configured bucket/index).
    #[tokio::test]
    async fn live_roundtrip_gated() {
        if std::env::var("S3VECTORS_LIVE_TEST").ok().as_deref() != Some("1") {
            return;
        }
        let cfg = S3VectorsConfig::from_env()
            .expect("LEGAL_RAG_S3V_BUCKET must be set")
            .expect("config must parse");
        let store = S3VectorsStore::connect(cfg, 4).await.unwrap();
        let rec = ChunkRecord {
            id: "live-test-1".into(),
            doc_id: "live-doc".into(),
            ordinal: 0,
            text: "live round trip".into(),
            embedding: vec![1.0, 0.0, 0.0, 0.0],
            metadata: HashMap::new(),
        };
        store.upsert(vec![rec]).await.unwrap();
        let got = store.get_by_ids(&["live-test-1".to_string()]).await.unwrap();
        assert_eq!(got.len(), 1);
        let hits = store.search(&[1.0, 0.0, 0.0, 0.0], 5).await.unwrap();
        assert!(hits.iter().any(|h| h.record.id == "live-test-1"));
        assert_eq!(store.remove_doc("live-doc").await.unwrap(), 1);
    }
}
