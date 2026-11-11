//! 外部 Webhook への HTTPS 送信 (署名・宛先検査・接続固定・リダイレクト禁止)。

use super::endpoint::{
    is_forbidden_ip, load_secret, parse_endpoint, parse_registration_endpoint, EndpointHost,
    EndpointMode, WebhookEndpoint, WebhookSecret, ENV_SECRET, ENV_SECRET_FILE, ENV_URL,
};
use super::signature;
use kernel::interfaces::repository::{WebhookRequest, WebhookSender};
use kernel::prelude::entity::{
    ValidatedWebhookUrl, WebhookConfigIssue, WebhookConfigStatus, WebhookDestination,
    WebhookEndpointRejection, WebhookSecret as KernelWebhookSecret, WebhookSendObservation,
};
use reqwest::header::{CONTENT_TYPE, RETRY_AFTER, USER_AGENT};
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const HEADER_EVENT_ID: &str = "x-emumet-event-id";
pub const HEADER_EVENT_TYPE: &str = "x-emumet-event-type";
pub const HEADER_DELIVERY_ATTEMPT: &str = "x-emumet-delivery-attempt";
pub const HEADER_TIMESTAMP: &str = "x-emumet-timestamp";
pub const HEADER_SIGNATURE: &str = "x-emumet-signature";

const WEBHOOK_USER_AGENT: &str = "Emumet-Webhook/1";

/// 送信の timeout と応答の読み捨て上限。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebhookNetworkSettings {
    pub resolve_timeout: Duration,
    pub connect_timeout: Duration,
    /// 接続・送信・応答受信を含む全体の timeout。配送 lease より十分短くすること。
    pub total_timeout: Duration,
    /// 応答本文は読み捨てる。読み捨てる最大バイト数 (超えた分は読まずに接続を閉じる)。
    pub max_response_bytes: usize,
}

impl Default for WebhookNetworkSettings {
    fn default() -> Self {
        Self {
            resolve_timeout: Duration::from_secs(5),
            connect_timeout: Duration::from_secs(5),
            total_timeout: Duration::from_secs(15),
            max_response_bytes: 64 * 1024,
        }
    }
}

type ResolveFuture<'a> =
    Pin<Box<dyn Future<Output = std::io::Result<Vec<SocketAddr>>> + Send + 'a>>;

/// 名前解決の差し替え点。解決結果は必ず宛先検査にかけられるので、差し替えても検査は弱まらない。
pub trait HostResolver: Send + Sync + 'static {
    fn resolve<'a>(&'a self, host: &'a str, port: u16) -> ResolveFuture<'a>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemResolver;

impl HostResolver for SystemResolver {
    fn resolve<'a>(&'a self, host: &'a str, port: u16) -> ResolveFuture<'a> {
        Box::pin(async move { Ok(tokio::net::lookup_host((host, port)).await?.collect()) })
    }
}

#[derive(Debug, Clone)]
struct EnabledConfig {
    endpoint: WebhookEndpoint,
    secret: WebhookSecret,
    mode: EndpointMode,
}

#[derive(Debug, Clone)]
enum ConfigState {
    Disabled,
    Misconfigured(WebhookConfigIssue),
    Enabled(Box<EnabledConfig>),
}

#[derive(Clone)]
pub struct HttpWebhookSender {
    state: ConfigState,
    /// 管理者 API で登録した送信先の宛先検査の厳しさ。production は常に `Strict`。
    endpoint_mode: EndpointMode,
    settings: WebhookNetworkSettings,
    resolver: Arc<dyn HostResolver>,
    #[cfg(any(test, feature = "test-mode"))]
    extra_roots: Vec<reqwest::Certificate>,
}

impl std::fmt::Debug for HttpWebhookSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpWebhookSender")
            .field("status", &self.status())
            .finish()
    }
}

fn build_state(
    url: Option<&str>,
    secret: Option<&str>,
    secret_file: Option<&str>,
    mode: EndpointMode,
) -> ConfigState {
    let url = url.map(str::trim).filter(|value| !value.is_empty());
    let secret = match load_secret(secret, secret_file) {
        Ok(secret) => secret,
        Err(issue) => return ConfigState::Misconfigured(issue),
    };
    match (url, secret) {
        (None, None) => ConfigState::Disabled,
        (None, Some(_)) => ConfigState::Misconfigured(WebhookConfigIssue::MissingUrl),
        (Some(_), None) => ConfigState::Misconfigured(WebhookConfigIssue::MissingSecret),
        (Some(url), Some(secret)) => match parse_endpoint(url, mode) {
            Ok(endpoint) => ConfigState::Enabled(Box::new(EnabledConfig {
                endpoint,
                secret,
                mode,
            })),
            Err(issue) => ConfigState::Misconfigured(issue),
        },
    }
}

impl HttpWebhookSender {
    /// 未設定 (Webhook 無効)。
    pub fn disabled() -> Self {
        Self::with_state(ConfigState::Disabled)
    }

    fn with_state(state: ConfigState) -> Self {
        Self {
            state,
            endpoint_mode: EndpointMode::Strict,
            settings: WebhookNetworkSettings::default(),
            resolver: Arc::new(SystemResolver),
            #[cfg(any(test, feature = "test-mode"))]
            extra_roots: Vec::new(),
        }
    }

    /// サーバー設定 (環境変数) から構築する。不備は `Misconfigured` 状態になるだけで失敗しない。
    pub fn from_env() -> Self {
        let url = dotenvy::var(ENV_URL).ok();
        let secret = dotenvy::var(ENV_SECRET).ok();
        let secret_file = dotenvy::var(ENV_SECRET_FILE).ok();
        Self::from_values(url.as_deref(), secret.as_deref(), secret_file.as_deref())
    }

    /// 設定値から構築する (production と同じ厳格な検査。http や内部宛先は許可されない)。
    pub fn from_values(url: Option<&str>, secret: Option<&str>, secret_file: Option<&str>) -> Self {
        Self::with_state(build_state(url, secret, secret_file, EndpointMode::Strict))
    }

    /// http・loopback 宛先を許可する検証用の構築。`cfg(test)` / `test-mode` ビルドにだけ存在し、
    /// 環境変数からは到達できない。
    #[cfg(any(test, feature = "test-mode"))]
    pub fn from_values_allowing_insecure_loopback(url: &str, secret: &str) -> Self {
        Self::with_state(build_state(
            Some(url),
            Some(secret),
            None,
            EndpointMode::AllowInsecureLoopback,
        ))
        .allowing_insecure_loopback_destinations()
    }

    /// 環境変数の送信先なし (無効) のまま、管理者 API で登録する送信先に http・loopback 宛先を許す。
    /// `cfg(test)` / `test-mode` ビルドにだけ存在し、設定・API からは到達できない。
    #[cfg(any(test, feature = "test-mode"))]
    pub fn disabled_allowing_insecure_loopback_destinations() -> Self {
        Self::disabled().allowing_insecure_loopback_destinations()
    }

    #[cfg(any(test, feature = "test-mode"))]
    fn allowing_insecure_loopback_destinations(mut self) -> Self {
        self.endpoint_mode = EndpointMode::AllowInsecureLoopback;
        self
    }

    /// 検証用に信頼する追加ルート証明書 (PEM)。TLS fixture テスト専用で、`cfg(test)` / `test-mode`
    /// ビルドにだけ存在する。
    #[cfg(any(test, feature = "test-mode"))]
    pub fn with_extra_root_certificate_pem(mut self, pem: &[u8]) -> Self {
        let certificate = reqwest::Certificate::from_pem(pem).expect("valid PEM certificate");
        self.extra_roots.push(certificate);
        self
    }

    pub fn with_settings(mut self, settings: WebhookNetworkSettings) -> Self {
        self.settings = settings;
        self
    }

    pub fn with_resolver(mut self, resolver: Arc<dyn HostResolver>) -> Self {
        self.resolver = resolver;
        self
    }

    async fn resolve_and_check(
        &self,
        endpoint: &WebhookEndpoint,
        mode: EndpointMode,
    ) -> Result<Vec<SocketAddr>, WebhookSendObservation> {
        let port = endpoint.port;
        let addresses = match &endpoint.host {
            EndpointHost::Ip(ip) => vec![SocketAddr::new(*ip, port)],
            EndpointHost::Domain(domain) => {
                let resolved = tokio::time::timeout(
                    self.settings.resolve_timeout,
                    self.resolver.resolve(domain, port),
                )
                .await;
                match resolved {
                    Ok(Ok(addresses)) if !addresses.is_empty() => addresses,
                    _ => return Err(WebhookSendObservation::DnsFailed),
                }
            }
        };
        if mode.is_strict()
            && addresses
                .iter()
                .any(|address| is_forbidden_ip(address.ip()))
        {
            return Err(WebhookSendObservation::SsrfBlocked);
        }
        Ok(addresses)
    }

    fn build_client(
        &self,
        config: &EnabledConfig,
        addresses: &[SocketAddr],
    ) -> Result<reqwest::Client, WebhookSendObservation> {
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .connect_timeout(self.settings.connect_timeout)
            .timeout(self.settings.total_timeout)
            .pool_max_idle_per_host(0)
            .https_only(config.mode.is_strict());
        #[cfg(any(test, feature = "test-mode"))]
        for certificate in &self.extra_roots {
            builder = builder.add_root_certificate(certificate.clone());
        }
        if let EndpointHost::Domain(domain) = &config.endpoint.host {
            builder = builder.resolve_to_addrs(domain, addresses);
        }
        builder
            .build()
            .map_err(|_| WebhookSendObservation::ConfigInvalid)
    }
}

pub(crate) fn parse_retry_after(value: &str, now: SystemTime) -> Option<time::Duration> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(time::Duration::seconds(
            i64::try_from(seconds).unwrap_or(i64::MAX),
        ));
    }
    let at = httpdate::parse_http_date(value).ok()?;
    let delta = match at.duration_since(now) {
        Ok(delta) => delta,
        Err(_) => return Some(time::Duration::ZERO),
    };
    Some(time::Duration::seconds(
        i64::try_from(delta.as_secs()).unwrap_or(i64::MAX),
    ))
}

impl WebhookSender for HttpWebhookSender {
    fn status(&self) -> WebhookConfigStatus {
        match &self.state {
            ConfigState::Disabled => WebhookConfigStatus::Disabled,
            ConfigState::Misconfigured(issue) => {
                WebhookConfigStatus::Misconfigured { issue: *issue }
            }
            ConfigState::Enabled(config) => WebhookConfigStatus::Enabled {
                host: config.endpoint.display_host(),
            },
        }
    }

    fn environment_destination(&self) -> Option<WebhookDestination> {
        let ConfigState::Enabled(config) = &self.state else {
            return None;
        };
        let secret =
            KernelWebhookSecret::from_trusted_bytes(config.secret.as_bytes().to_vec()).ok()?;
        Some(WebhookDestination {
            url: config.endpoint.url.to_string(),
            secret,
        })
    }

    async fn validate_destination(
        &self,
        raw_url: &str,
    ) -> Result<ValidatedWebhookUrl, WebhookEndpointRejection> {
        let endpoint = parse_registration_endpoint(raw_url, self.endpoint_mode)?;
        match self.resolve_and_check(&endpoint, self.endpoint_mode).await {
            Ok(_) => Ok(ValidatedWebhookUrl::new_checked(endpoint.url.to_string())),
            Err(WebhookSendObservation::SsrfBlocked) => {
                Err(WebhookEndpointRejection::ForbiddenDestination)
            }
            Err(_) => Err(WebhookEndpointRejection::HostUnresolvable),
        }
    }

    async fn send_to(
        &self,
        destination: &WebhookDestination,
        request: &WebhookRequest<'_>,
    ) -> WebhookSendObservation {
        let endpoint = match parse_endpoint(&destination.url, self.endpoint_mode) {
            Ok(endpoint) => endpoint,
            Err(WebhookConfigIssue::ForbiddenDestination) => {
                return WebhookSendObservation::SsrfBlocked
            }
            Err(_) => return WebhookSendObservation::ConfigInvalid,
        };
        let Ok(secret) = WebhookSecret::new(destination.secret.expose_bytes().to_vec()) else {
            return WebhookSendObservation::ConfigInvalid;
        };
        let config = EnabledConfig {
            endpoint,
            secret,
            mode: self.endpoint_mode,
        };
        self.send_with_config(&config, request).await
    }

    async fn send(&self, request: &WebhookRequest<'_>) -> WebhookSendObservation {
        let ConfigState::Enabled(config) = &self.state else {
            return WebhookSendObservation::ConfigInvalid;
        };
        self.send_with_config(config, request).await
    }
}

impl HttpWebhookSender {
    /// 接続時の検査 (毎回): 名前解決した全アドレスを再検査し、検査したアドレスへ接続を固定する。
    async fn send_with_config(
        &self,
        config: &EnabledConfig,
        request: &WebhookRequest<'_>,
    ) -> WebhookSendObservation {
        let addresses = match self.resolve_and_check(&config.endpoint, config.mode).await {
            Ok(addresses) => addresses,
            Err(observation) => return observation,
        };
        let client = match self.build_client(config, &addresses) {
            Ok(client) => client,
            Err(observation) => return observation,
        };

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX))
            .unwrap_or(0);
        let signature = signature::sign(config.secret.as_bytes(), timestamp, request.body);

        let response = client
            .post(config.endpoint.url.clone())
            .header(CONTENT_TYPE, "application/json")
            .header(USER_AGENT, WEBHOOK_USER_AGENT)
            .header(HEADER_EVENT_ID, request.event_id)
            .header(HEADER_EVENT_TYPE, request.event_type.as_str())
            .header(HEADER_DELIVERY_ATTEMPT, request.attempt.to_string())
            .header(HEADER_TIMESTAMP, timestamp.to_string())
            .header(HEADER_SIGNATURE, signature)
            .body(request.body.to_vec())
            .send()
            .await;

        let mut response = match response {
            Ok(response) => response,
            Err(error) if error.is_timeout() => return WebhookSendObservation::Timeout,
            Err(_) => return WebhookSendObservation::ConnectFailed,
        };

        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| parse_retry_after(value, SystemTime::now()));

        let mut drained = 0usize;
        while drained < self.settings.max_response_bytes {
            match response.chunk().await {
                Ok(Some(chunk)) => drained += chunk.len(),
                _ => break,
            }
        }

        WebhookSendObservation::Response {
            status,
            retry_after,
        }
    }
}
