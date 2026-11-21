//! Post-relay transport: no database reference and no payload/response/error logging.
#[cfg(test)]
mod tests;
use crate::home_origin::HomeOriginAllowlist;
use crate::profile_delivery::target::{
    build_client, resolve_target, ProfileDeliveryNetworkSettings, TargetError, TargetMode,
};
use crate::webhook::{parse_retry_after, HostResolver, SystemResolver};
use kernel::interfaces::post_relay::{PostRelayTransport, RelayHttpRequest, RelayHttpResult};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

// Cache only clients bound to the EXACT freshly validated address set. DNS is checked on
// every attempt before consulting this bounded cache, so rebinding cannot reuse a public
// approval for a private address. No payloads/keys. Avoid per-attempt native-TLS root loading.
struct PinnedClient {
    origin: String,
    addresses: Vec<std::net::SocketAddr>,
    client: reqwest::Client,
    touched: std::time::Instant,
}
#[derive(Clone)]
pub struct HttpPostRelayTransport {
    policy: HomeOriginAllowlist,
    clients: Arc<tokio::sync::Mutex<Vec<PinnedClient>>>,
    #[cfg(any(test, feature = "test-mode"))]
    test_origins: Vec<String>,
    resolver: Arc<dyn HostResolver>,
    mode: TargetMode,
}
impl HttpPostRelayTransport {
    pub fn new(policy: HomeOriginAllowlist) -> Self {
        Self {
            policy,
            clients: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            #[cfg(any(test, feature = "test-mode"))]
            test_origins: Vec::new(),
            resolver: Arc::new(SystemResolver),
            mode: TargetMode::Strict,
        }
    }
    #[cfg(any(test, feature = "test-mode"))]
    pub fn with_test_origins(mut self, origins: Vec<String>) -> Self {
        self.test_origins = origins;
        self.mode = TargetMode::AllowInsecureLoopback;
        self
    }
    pub fn with_resolver(mut self, resolver: Arc<dyn HostResolver>) -> Self {
        self.resolver = resolver;
        self
    }
    #[cfg(any(test, feature = "test-mode"))]
    pub fn allowing_insecure_loopback(mut self) -> Self {
        self.mode = TargetMode::AllowInsecureLoopback;
        self
    }
}
impl PostRelayTransport for HttpPostRelayTransport {
    fn origin_allowed(&self, origin: &str) -> bool {
        #[cfg(any(test, feature = "test-mode"))]
        if self.test_origins.iter().any(|o| o == origin) {
            return true;
        }
        self.policy.contains_origin(origin)
    }
    async fn send(&self, request: &RelayHttpRequest<'_>) -> RelayHttpResult {
        // A timeout never restarts this budget. Even DNS and a stalled resolver are inside it.
        let deadline = tokio::time::Instant::from_std(request.deadline);
        if tokio::time::Instant::now() >= deadline {
            return RelayHttpResult::Transient;
        }
        let work = async {
            let settings = ProfileDeliveryNetworkSettings {
                resolve_timeout: Duration::from_secs(10),
                connect_timeout: Duration::from_secs(10),
                total_timeout: Duration::from_secs(10),
                max_response_bytes: 0,
            };
            if !reqwest::Url::parse(request.origin).is_ok_and(|u| {
                u.host_str().is_some()
                    && u.username().is_empty()
                    && u.password().is_none()
                    && u.path() == "/"
                    && u.query().is_none()
                    && u.fragment().is_none()
            }) {
                return RelayHttpResult::InvalidTarget;
            }
            let url = format!("{}/internal/v1/emumet/activities", request.origin);
            let target = match resolve_target(&url, self.mode, &self.resolver, &settings).await {
                Ok(t) => t,
                Err(TargetError::Invalid) => return RelayHttpResult::InvalidTarget,
                Err(TargetError::DnsFailed) => return RelayHttpResult::Transient,
            };
            let client = {
                let mut clients = self.clients.lock().await;
                clients.retain(|c| c.touched.elapsed() < Duration::from_secs(60));
                if let Some(c) = clients
                    .iter_mut()
                    .find(|c| c.origin == request.origin && c.addresses == target.addresses)
                {
                    c.touched = std::time::Instant::now();
                    c.client.clone()
                } else {
                    let Some(client) = build_client(&target, self.mode, &settings) else {
                        return RelayHttpResult::InvalidTarget;
                    };
                    if clients.len() >= 64 {
                        let oldest = clients
                            .iter()
                            .enumerate()
                            .min_by_key(|(_, c)| c.touched)
                            .map(|(i, _)| i)
                            .expect("nonempty");
                        clients.swap_remove(oldest);
                    }
                    clients.push(PinnedClient {
                        origin: request.origin.into(),
                        addresses: target.addresses.clone(),
                        client: client.clone(),
                        touched: std::time::Instant::now(),
                    });
                    client
                }
            };
            let mut headers = reqwest::header::HeaderMap::new();
            for (name, value) in request.headers {
                let (Ok(name), Ok(value)) = (
                    reqwest::header::HeaderName::from_bytes(name.as_bytes()),
                    reqwest::header::HeaderValue::from_str(value),
                ) else {
                    return RelayHttpResult::InvalidTarget;
                };
                headers.insert(name, value);
            }
            let outgoing = match client
                .post(target.url)
                .headers(headers)
                .body(request.envelope.to_vec())
                .build()
            {
                Ok(r) => r,
                Err(_) => return RelayHttpResult::InvalidTarget,
            };
            // Synchronous client/header/body construction also consumes the committed budget.
            if tokio::time::Instant::now() >= deadline {
                return RelayHttpResult::Transient;
            }
            match client.execute(outgoing).await {
                Ok(response) => {
                    let status = response.status().as_u16();
                    let retry_after_secs = if matches!(status, 429 | 503) {
                        response
                            .headers()
                            .get(reqwest::header::RETRY_AFTER)
                            .and_then(|v| v.to_str().ok())
                            .and_then(|v| parse_retry_after(v, SystemTime::now()))
                            .map(|d| d.whole_seconds().max(0) as u64)
                    } else {
                        None
                    };
                    // Never read an unbounded response body. Status/headers constitute observation.
                    RelayHttpResult::Response {
                        status,
                        retry_after_secs,
                    }
                }
                Err(_) => RelayHttpResult::Transient,
            }
        };
        tokio::time::timeout_at(deadline, work)
            .await
            .unwrap_or(RelayHttpResult::Transient)
    }
}
