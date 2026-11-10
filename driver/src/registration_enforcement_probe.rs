//! Black-box evidence from public Kratos registration, not its health endpoint.
use error_stack::Report;
use kernel::interfaces::repository::{
    EnforcementProbeFlow, EnforcementProbeId, EnforcementProbeMode,
    RegistrationEnforcementProbeClient,
};
use kernel::KernelError;
use reqwest::{header, Client, Response, Url};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Duration};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeTargetSettings {
    pub public_url: String,
    pub admin_url: String,
}

struct Target {
    public: Url,
    admin: Url,
    key: String,
}

/// No Debug: flow responses and browser cookies are never logged.
#[derive(Clone)]
pub struct HttpKratosEnforcementProbe {
    client: Client,
    targets: Arc<Vec<Target>>,
    generation: String,
}

fn failure() -> Report<KernelError> {
    Report::new(KernelError::Internal).attach_printable("Kratos enforcement probe did not complete")
}

fn digest(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn base_url(value: &str) -> Option<Url> {
    let mut url = Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    if !url.path().ends_with('/') {
        url.set_path(&format!("{}/", url.path()));
    }
    Some(url)
}

async fn bounded_json(mut response: Response) -> error_stack::Result<Value, KernelError> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| failure())? {
        if bytes.len() + chunk.len() > 64 * 1024 {
            return Err(failure());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| failure())
}

impl HttpKratosEnforcementProbe {
    pub fn from_env() -> Self {
        let settings = match std::env::var("REGISTRATION_ENFORCEMENT_PROBE_TARGETS") {
            Ok(value) => {
                serde_json::from_str::<Vec<ProbeTargetSettings>>(&value).unwrap_or_default()
            }
            Err(std::env::VarError::NotPresent) => vec![ProbeTargetSettings {
                public_url: std::env::var("KRATOS_PUBLIC_URL")
                    .unwrap_or_else(|_| "http://localhost:4433/".to_string()),
                admin_url: std::env::var("KRATOS_ADMIN_URL")
                    .unwrap_or_else(|_| "http://localhost:4434/".to_string()),
            }],
            Err(_) => Vec::new(),
        };
        let mut client = Self::new(settings);
        let period = std::env::var("REGISTRATION_ENFORCEMENT_PROBE_PERIOD_SECS")
            .unwrap_or_else(|_| "30".to_string());
        client.generation = digest(&format!("{}\nperiod={period}", client.generation));
        client
    }

    pub fn new(settings: Vec<ProbeTargetSettings>) -> Self {
        let targets = (1..=4)
            .contains(&settings.len())
            .then(|| {
                settings
                    .into_iter()
                    .map(|target| {
                        let public = base_url(&target.public_url)?;
                        let admin = base_url(&target.admin_url)?;
                        let key = digest(&format!("{public}\n{admin}"));
                        Some(Target { public, admin, key })
                    })
                    .collect::<Option<Vec<_>>>()
            })
            .flatten()
            .unwrap_or_default();
        let generation = digest(&format!(
            "kratos-password-pre-persist-negative-v1\n{}",
            targets
                .iter()
                .map(|t| t.key.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        ));
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(5))
                .connect_timeout(Duration::from_secs(1))
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .build()
                .expect("static HTTP client configuration"),
            targets: Arc::new(targets),
            generation,
        }
    }

    fn target(&self, key: &str) -> error_stack::Result<&Target, KernelError> {
        self.targets
            .iter()
            .find(|target| target.key == key)
            .ok_or_else(failure)
    }

    fn url(base: &Url, path: &str) -> error_stack::Result<Url, KernelError> {
        base.join(path).map_err(|_| failure())
    }

    fn valid_identity_id(id: &str) -> bool {
        id.len() == 36
            && id.bytes().enumerate().all(|(i, b)| {
                if [8, 13, 18, 23].contains(&i) {
                    b == b'-'
                } else {
                    b.is_ascii_hexdigit()
                }
            })
    }
}

impl RegistrationEnforcementProbeClient for HttpKratosEnforcementProbe {
    fn generation(&self) -> &str {
        &self.generation
    }

    fn target_keys(&self) -> Vec<String> {
        self.targets
            .iter()
            .map(|target| target.key.clone())
            .collect()
    }

    async fn initialize(
        &self,
        target_key: &str,
        mode: EnforcementProbeMode,
    ) -> error_stack::Result<EnforcementProbeFlow, KernelError> {
        let target = self.target(target_key)?;
        let url = Self::url(
            &target.public,
            &format!("self-service/registration/{}", mode.as_str()),
        )?;
        let response = self
            .client
            .get(url)
            .header(header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|_| failure())?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(failure());
        }
        let cookie = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .filter_map(|value| value.split(';').next())
            .filter(|pair| pair.starts_with("csrf_token_") && pair.len() <= 512)
            .collect::<Vec<_>>()
            .join("; ");
        let body = bounded_json(response).await?;
        let flow_id = body["id"]
            .as_str()
            .filter(|id| Self::valid_identity_id(id))
            .ok_or_else(failure)?;
        if body["type"].as_str() != Some(mode.as_str()) {
            return Err(failure());
        }
        let nodes = body["ui"]["nodes"].as_array().ok_or_else(failure)?;
        // Profile is the credential-selection screen, not a credential registration strategy.
        if nodes.iter().any(|node| {
            !matches!(
                node["group"].as_str(),
                Some("default" | "profile" | "password")
            )
        }) {
            return Err(failure());
        }
        let csrf = nodes
            .iter()
            .find(|node| node["attributes"]["name"] == "csrf_token")
            .and_then(|node| node["attributes"]["value"].as_str())
            .map(str::to_string);
        if mode == EnforcementProbeMode::Browser && (cookie.is_empty() || csrf.is_none()) {
            return Err(failure());
        }
        Ok(EnforcementProbeFlow {
            target_key: target_key.to_string(),
            flow_id: flow_id.to_string(),
            mode,
            csrf_cookie: (!cookie.is_empty()).then_some(cookie),
            csrf_token: csrf,
        })
    }

    async fn submit(
        &self,
        flow: &EnforcementProbeFlow,
        probe_id: &EnforcementProbeId,
    ) -> error_stack::Result<bool, KernelError> {
        let target = self.target(&flow.target_key)?;
        let mut url = Self::url(&target.public, "self-service/registration")?;
        url.query_pairs_mut().append_pair("flow", &flow.flow_id);
        let mut body = json!({
            "method": "password",
            "password": format!("Canary-only-{}-x9!Z", probe_id.as_str()),
            "traits": {"email": probe_id.email()},
            "transient_payload": {"registration_probe_id": probe_id.as_str()}
        });
        if let Some(csrf) = &flow.csrf_token {
            body["csrf_token"] = json!(csrf);
        }
        let mut request = self
            .client
            .post(url)
            .header(header::ACCEPT, "application/json")
            .header(header::ORIGIN, target.public.origin().ascii_serialization())
            .json(&body);
        if let Some(cookie) = &flow.csrf_cookie {
            request = request.header(header::COOKIE, cookie);
        }
        let response = request.send().await.map_err(|_| failure())?;
        if response.status() != reqwest::StatusCode::BAD_REQUEST {
            return Ok(false);
        }
        let body = bounded_json(response).await?;
        Ok(body["ui"]["messages"].as_array().is_some_and(|messages| {
            messages
                .iter()
                .any(|message| message["id"].as_i64() == Some(4_000_001))
        }))
    }

    async fn created_identities(
        &self,
        target_key: &str,
        probe_id: &EnforcementProbeId,
    ) -> error_stack::Result<Vec<String>, KernelError> {
        let target = self.target(target_key)?;
        let mut url = Self::url(&target.admin, "admin/identities")?;
        url.query_pairs_mut()
            .append_pair("credentials_identifier", &probe_id.email())
            .append_pair("page_size", "2")
            .append_pair("consistency", "strong");
        let response = self.client.get(url).send().await.map_err(|_| failure())?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(failure());
        }
        let body = bounded_json(response).await?;
        let identities = body.as_array().ok_or_else(failure)?;
        if identities.len() > 1 {
            return Err(failure());
        }
        identities
            .iter()
            .map(|identity| {
                if identity["traits"]["email"].as_str() != Some(probe_id.email().as_str()) {
                    return Err(failure());
                }
                identity["id"]
                    .as_str()
                    .filter(|id| Self::valid_identity_id(id))
                    .map(str::to_string)
                    .ok_or_else(failure)
            })
            .collect()
    }

    async fn delete_identity(
        &self,
        target_key: &str,
        probe_id: &EnforcementProbeId,
        identity_id: &str,
    ) -> error_stack::Result<(), KernelError> {
        if !Self::valid_identity_id(identity_id) {
            return Err(failure());
        }
        let target = self.target(target_key)?;
        let url = Self::url(&target.admin, &format!("admin/identities/{identity_id}"))?;
        let response = self
            .client
            .get(url.clone())
            .send()
            .await
            .map_err(|_| failure())?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        if response.status() != reqwest::StatusCode::OK {
            return Err(failure());
        }
        let body = bounded_json(response).await?;
        if body["traits"]["email"].as_str() != Some(probe_id.email().as_str()) {
            return Err(failure());
        }
        let response = self
            .client
            .delete(url)
            .send()
            .await
            .map_err(|_| failure())?;
        if !matches!(
            response.status(),
            reqwest::StatusCode::NO_CONTENT | reqwest::StatusCode::NOT_FOUND
        ) {
            return Err(failure());
        }
        Ok(())
    }
}
