use reqwest::Client;
use serde::Deserialize;
use url::Url;

pub struct KratosClient {
    public_url: String,
    http_client: Client,
}

impl KratosClient {
    /// Create a new KratosClient. Panics if `public_url` is not a valid URL.
    pub fn new(public_url: String) -> Self {
        let public_url = public_url.trim_end_matches('/').to_string();
        Url::parse(&public_url)
            .unwrap_or_else(|e| panic!("KRATOS_PUBLIC_URL is not a valid URL ({public_url}): {e}"));
        Self {
            public_url,
            http_client: Client::new(),
        }
    }

    /// Kratos の /sessions/whoami エンドポイントを呼び出し、
    /// 有効なセッションがあれば KratosSession を返す。
    /// セッションがない場合（401）は None を返す。
    pub async fn whoami(&self, cookie: &str) -> Result<Option<KratosSession>, reqwest::Error> {
        let url = format!("{}/sessions/whoami", self.public_url);
        tracing::debug!("Calling Kratos whoami: url={url}");

        let response = self
            .http_client
            .get(&url)
            .header("cookie", cookie)
            .send()
            .await?;

        let status = response.status();
        tracing::debug!("Kratos whoami response: status={status}");

        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Ok(None);
        }

        let session = response.error_for_status()?.json::<KratosSession>().await?;
        tracing::debug!(
            "Kratos whoami session: id={}, active={}",
            session.id,
            session.active
        );
        if !session.active {
            tracing::debug!("Kratos whoami: session is not active, treating as unauthenticated");
            return Ok(None);
        }
        Ok(Some(session))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct KratosSession {
    pub id: String,
    #[serde(default)]
    pub active: bool,
    pub identity: KratosIdentity,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)] // JSONデシリアライズで全フィールドが必要
pub struct KratosIdentity {
    pub id: String,
    #[serde(default)]
    pub traits: serde_json::Value,
}
