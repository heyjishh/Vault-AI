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
    endpoint_url: Option<String>,
}

impl S3Source {
    pub async fn connect(bucket: &str, region: &str) -> Result<Self> {
        Self::connect_with_endpoint(bucket, region, None).await
    }

    /// Connect to an S3-compatible store. With `endpoint_url` set
    /// (Cloudflare R2, MinIO, …), the SDK client is built with that
    /// endpoint and path-style addressing forced, so requests go to
    /// `<endpoint>/<bucket>/<key>` instead of
    /// `<bucket>.s3.<region>.amazonaws.com/<key>`. Region is still
    /// required: it scopes request signing (R2 users set it to
    /// `auto`). With `None`, behavior is stock AWS, unchanged.
    pub async fn connect_with_endpoint(
        bucket: &str,
        region: &str,
        endpoint_url: Option<&str>,
    ) -> Result<Self> {
        let sdk_config = aws_config::defaults(BehaviorVersion::latest())
            .region(Region::new(region.to_string()))
            .load()
            .await;
        let client = match endpoint_url {
            Some(url) => {
                let conf = aws_sdk_s3::config::Builder::from(&sdk_config)
                    .endpoint_url(url.to_string())
                    .force_path_style(true)
                    .build();
                Client::from_conf(conf)
            }
            None => Client::new(&sdk_config),
        };
        Ok(Self {
            client,
            bucket: bucket.to_string(),
            region: region.to_string(),
            endpoint_url: endpoint_url.map(|u| u.to_string()),
        })
    }

    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    pub fn region(&self) -> &str {
        &self.region
    }

    pub fn endpoint_url(&self) -> Option<&str> {
        self.endpoint_url.as_deref()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connect_with_endpoint_keeps_accessors_and_defaults_to_aws() {
        // Hermetic: dummy env credentials + IMDS off so config load
        // never touches the network. No live R2/S3 call is made here
        // or anywhere in this test — live verification needs real
        // credentials, which do not exist in this environment.
        std::env::set_var("AWS_ACCESS_KEY_ID", "test");
        std::env::set_var("AWS_SECRET_ACCESS_KEY", "test");
        std::env::set_var("AWS_EC2_METADATA_DISABLED", "true");
        let r2 = S3Source::connect_with_endpoint(
            "vault",
            "auto",
            Some("https://example.r2.cloudflarestorage.com"),
        )
        .await
        .unwrap();
        assert_eq!(r2.bucket(), "vault");
        assert_eq!(r2.region(), "auto");
        assert_eq!(
            r2.endpoint_url(),
            Some("https://example.r2.cloudflarestorage.com")
        );
        let aws = S3Source::connect("vault", "ap-south-1").await.unwrap();
        assert_eq!(aws.bucket(), "vault");
        assert_eq!(aws.endpoint_url(), None);
    }
}
