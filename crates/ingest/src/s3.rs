//! S3 object source for the vault.

use aws_config::BehaviorVersion;
use aws_sdk_s3::config::Region;
use aws_sdk_s3::Client;
use legal_rag_common::{Error, Result};
use tracing::info;

/// Lists and downloads objects from an S3 bucket.
#[derive(Clone)]
pub struct S3Source {
    client: Client,
    bucket: String,
    region: String,
}

impl S3Source {
    pub async fn connect(bucket: &str, region: &str) -> Result<Self> {
        let sdk_config = aws_config::defaults(BehaviorVersion::latest())
            .region(Region::new(region.to_string()))
            .load()
            .await;
        Ok(Self {
            client: Client::new(&sdk_config),
            bucket: bucket.to_string(),
            region: region.to_string(),
        })
    }

    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    pub fn region(&self) -> &str {
        &self.region
    }

    /// List object keys under a prefix (paginated).
    pub async fn list_keys(&self, prefix: &str) -> Result<Vec<String>> {
        let mut keys = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut req = self.client.list_objects_v2().bucket(&self.bucket).prefix(prefix);
            if let Some(t) = token {
                req = req.continuation_token(t);
            }
            let resp = req.send().await.map_err(|e| Error::Upstream(format!("s3 list failed: {e}")))?;
            for obj in resp.contents() {
                if let Some(key) = obj.key() {
                    if !key.ends_with('/') {
                        keys.push(key.to_string());
                    }
                }
            }
            if resp.is_truncated() == Some(true) {
                token = resp.next_continuation_token().map(|s| s.to_string());
            } else {
                break;
            }
        }
        info!(bucket = %self.bucket, prefix = %prefix, count = keys.len(), "listed s3 objects");
        Ok(keys)
    }

    /// Download one object fully into memory.
    pub async fn download(&self, key: &str) -> Result<Vec<u8>> {
        let resp = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| Error::Upstream(format!("s3 get failed for {key}: {e}")))?;
        let bytes = resp
            .body
            .collect()
            .await
            .map_err(|e| Error::Upstream(format!("s3 body read failed: {e}")))?
            .into_bytes()
            .to_vec();
        Ok(bytes)
    }
}
