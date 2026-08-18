use aws_config::{BehaviorVersion, Region};
use aws_sdk_s3::config::Credentials;
use aws_sdk_s3::primitives::ByteStream;
use error_stack::Report;
use kernel::interfaces::storage::{ImageStorage, StoredObject};
use kernel::KernelError;

#[derive(Clone)]
pub struct S3ImageStorage {
    client: aws_sdk_s3::Client,
    bucket: String,
    public_base_url: String,
}

impl S3ImageStorage {
    pub async fn from_env() -> error_stack::Result<Self, KernelError> {
        let endpoint = env_or("S3_ENDPOINT", "http://localhost:9000");
        let region_name = env_or("S3_REGION", "us-east-1");
        let bucket_name = env_or("S3_BUCKET", "emumet-media");
        let access_key = env_or("S3_ACCESS_KEY", "emumetdevelop");
        let secret_key = env_or("S3_SECRET_KEY", "emumet-develop-secret");
        let public_base_url = env_or(
            "MEDIA_PUBLIC_BASE_URL",
            "http://localhost:9000/emumet-media",
        )
        .trim_end_matches('/')
        .to_string();
        let credentials = Credentials::new(access_key, secret_key, None, None, "emumet-env");
        let shared_config = aws_config::defaults(BehaviorVersion::latest())
            .region(Region::new(region_name))
            .credentials_provider(credentials)
            .load()
            .await;
        let config = aws_sdk_s3::config::Builder::from(&shared_config)
            .endpoint_url(endpoint)
            .force_path_style(true)
            .build();

        Ok(Self {
            client: aws_sdk_s3::Client::from_conf(config),
            bucket: bucket_name,
            public_base_url,
        })
    }
}

impl ImageStorage for S3ImageStorage {
    async fn put(
        &self,
        key: &str,
        content_type: &str,
        bytes: &[u8],
    ) -> error_stack::Result<StoredObject, KernelError> {
        if self
            .client
            .head_bucket()
            .bucket(&self.bucket)
            .send()
            .await
            .is_err()
        {
            self.client
                .create_bucket()
                .bucket(&self.bucket)
                .send()
                .await
                .map_err(|error| {
                    Report::new(KernelError::Internal)
                        .attach_printable(format!("Failed to create S3 bucket: {error}"))
                })?;
            let policy = format!(
                r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Principal":{{"AWS":["*"]}},"Action":["s3:GetObject"],"Resource":["arn:aws:s3:::{bucket}/*"]}}]}}"#,
                bucket = self.bucket
            );
            self.client
                .put_bucket_policy()
                .bucket(&self.bucket)
                .policy(policy)
                .send()
                .await
                .map_err(|error| {
                    Report::new(KernelError::Internal)
                        .attach_printable(format!("Failed to set S3 bucket policy: {error}"))
                })?;
        }
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .content_type(content_type)
            .body(ByteStream::from(bytes.to_vec()))
            .send()
            .await
            .map_err(|error| {
                Report::new(KernelError::Internal)
                    .attach_printable(format!("Failed to upload image to S3: {error}"))
            })?;
        Ok(StoredObject {
            key: key.to_string(),
            url: format!("{}/{}", self.public_base_url, key.trim_start_matches('/')),
        })
    }
}

fn env_or(name: &str, default: &str) -> String {
    dotenvy::var(name).unwrap_or_else(|_| default.to_string())
}
