//! Cloudflare Turnstile Siteverify adapter。
//!
//! `POST https://challenges.cloudflare.com/turnstile/v0/siteverify` を HTTPS 固定・redirect 追従なし・
//! 短い timeout・応答本文サイズ上限で呼び、応答を契約の分類に写像する。HTTP 200 でも `success:false`
//! は失敗で、`success:true` でも expected hostname/action・token の期限を照合する。
//! secret・token・応答本文はログ・エラー・分類に出さない。

use super::config::{
    derive_generation, explicit_enabled, load_secret, non_empty, secret_fingerprint, Secret,
    SecretError, ValueSource,
};
use kernel::interfaces::repository::{BotTokenCheck, TurnstileVerifier};
use kernel::prelude::entity::{
    Protection, ProtectionConfigSnapshot, ProtectionProvider, ProviderResult, ReasonCode,
    TransportClass,
};
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

pub const SITEVERIFY_URL: &str = "https://challenges.cloudflare.com/turnstile/v0/siteverify";

pub const ENV_ENABLED: &str = "REGISTRATION_TURNSTILE_ENABLED";
pub const ENV_SITE_KEY: &str = "REGISTRATION_TURNSTILE_SITE_KEY";
pub const ENV_SECRET: &str = "REGISTRATION_TURNSTILE_SECRET";
pub const ENV_SECRET_FILE: &str = "REGISTRATION_TURNSTILE_SECRET_FILE";
pub const ENV_EXPECTED_HOSTNAME: &str = "REGISTRATION_TURNSTILE_EXPECTED_HOSTNAME";
pub const ENV_EXPECTED_ACTION: &str = "REGISTRATION_TURNSTILE_EXPECTED_ACTION";
pub const ENV_TOKEN_MAX_AGE_SECS: &str = "REGISTRATION_TURNSTILE_TOKEN_MAX_AGE_SECS";

/// Turnstile の token の有効期間は 300 秒。
const DEFAULT_TOKEN_MAX_AGE_SECS: i64 = 300;
/// readiness probe が送る、有効でない固定の token (provider が secret を受理するかだけを見る)。
const PROBE_TOKEN: &str = "emumet-readiness-probe-not-a-token";
const MAX_TOKEN_LEN: usize = 2048;

/// timeout と応答サイズの上限。hook 全体 5 秒以内に収まるよう provider deadline は短く取る。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TurnstileNetworkSettings {
    pub connect_timeout: Duration,
    pub total_timeout: Duration,
    pub max_response_bytes: usize,
}

impl Default for TurnstileNetworkSettings {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(2),
            total_timeout: Duration::from_secs(3),
            max_response_bytes: 16 * 1024,
        }
    }
}

/// 照合する期待値。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteverifyExpectation {
    pub hostname: String,
    pub action: Option<String>,
    pub token_max_age: time::Duration,
}

/// 読み込んだ設定 (secret を含むので Debug は伏せる)。
pub struct TurnstileSettings {
    snapshot: ProtectionConfigSnapshot,
    secret: Option<Secret>,
    expectation: SiteverifyExpectation,
}

impl std::fmt::Debug for TurnstileSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TurnstileSettings")
            .field("enabled", &self.snapshot.enabled)
            .field("issue", &self.snapshot.issue)
            .field("generation", &self.snapshot.generation)
            .finish()
    }
}

impl TurnstileSettings {
    pub fn from_env() -> Self {
        Self::from_source(&super::config::ProcessEnv)
    }

    /// 設定値から構築する。不備は失敗せず `snapshot().issue` に分類される。
    pub fn from_source(source: &dyn ValueSource) -> Self {
        let site_key = non_empty(source, ENV_SITE_KEY);
        let hostname = non_empty(source, ENV_EXPECTED_HOSTNAME);
        let action = non_empty(source, ENV_EXPECTED_ACTION);
        let max_age_raw = non_empty(source, ENV_TOKEN_MAX_AGE_SECS);
        let explicit = explicit_enabled(source, ENV_ENABLED);
        let secret = load_secret(source, ENV_SECRET, ENV_SECRET_FILE);

        let any_present = site_key.is_some()
            || hostname.is_some()
            || action.is_some()
            || max_age_raw.is_some()
            || non_empty(source, ENV_SECRET).is_some()
            || non_empty(source, ENV_SECRET_FILE).is_some();

        let max_age_secs = match max_age_raw.as_deref() {
            None => Some(DEFAULT_TOKEN_MAX_AGE_SECS),
            Some(raw) => raw.parse::<i64>().ok().filter(|v| (1..=3600).contains(v)),
        };

        let (enabled, mut issue) = match explicit {
            Some(false) => (false, Some(ReasonCode::Disabled)),
            _ if !any_present => (false, Some(ReasonCode::NotConfigured)),
            _ => (true, None),
        };
        let secret_value = match secret {
            Ok(secret) => secret,
            Err(SecretError::Ambiguous) => {
                issue = issue.or(Some(ReasonCode::InvalidConfiguration));
                None
            }
            Err(SecretError::Unreadable) => {
                issue = issue.or(Some(ReasonCode::MissingSecret));
                None
            }
        };
        if enabled && issue.is_none() {
            if secret_value.is_none() {
                issue = Some(ReasonCode::MissingSecret);
            } else if hostname.is_none() || max_age_secs.is_none() {
                issue = Some(ReasonCode::InvalidConfiguration);
            }
        }

        let fingerprint = secret_value
            .as_ref()
            .map(|secret| secret_fingerprint(secret.as_bytes()))
            .unwrap_or_default();
        let max_age_text = max_age_secs.unwrap_or(0).to_string();
        let generation = derive_generation(
            "bot",
            ProtectionProvider::Turnstile.as_str(),
            enabled,
            &[
                ("site_key", site_key.as_deref().unwrap_or("")),
                ("expected_hostname", hostname.as_deref().unwrap_or("")),
                ("expected_action", action.as_deref().unwrap_or("")),
                ("token_max_age_secs", &max_age_text),
            ],
            &fingerprint,
        );
        Self {
            snapshot: ProtectionConfigSnapshot {
                protection: Protection::Bot,
                provider: ProtectionProvider::Turnstile,
                enabled,
                issue,
                generation,
                fingerprint,
            },
            secret: secret_value,
            expectation: SiteverifyExpectation {
                hostname: hostname.unwrap_or_default(),
                action,
                token_max_age: time::Duration::seconds(
                    max_age_secs.unwrap_or(DEFAULT_TOKEN_MAX_AGE_SECS),
                ),
            },
        }
    }

    pub fn snapshot(&self) -> &ProtectionConfigSnapshot {
        &self.snapshot
    }
}

#[derive(Deserialize)]
struct SiteverifyBody {
    success: Option<bool>,
    #[serde(default, rename = "error-codes")]
    error_codes: Vec<String>,
    hostname: Option<String>,
    action: Option<String>,
    challenge_ts: Option<String>,
}

fn transport_for_status(status: u16) -> TransportClass {
    match status {
        200..=299 => TransportClass::Http2xx,
        429 => TransportClass::Http429,
        400..=499 => TransportClass::Http4xx,
        500..=599 => TransportClass::Http5xx,
        _ => TransportClass::HttpOther,
    }
}

/// Siteverify の応答 (HTTP status と本文) を契約の分類へ写像する純粋関数。
///
/// - 429 / 5xx / 解釈できない応答 → `temporarily_unavailable`
/// - `success:false` は HTTP 200 でも失敗。secret の error-codes は `configuration_invalid`、
///   利用者 token 起因 (invalid / missing / duplicate) は `attempt_rejected` (provider 全体を down にしない)
/// - `success:true` は hostname / action / token の期限を照合し、すべて合致して初めて business success
pub fn classify_siteverify(
    http_status: u16,
    body: &[u8],
    expected: &SiteverifyExpectation,
    now: OffsetDateTime,
) -> ProviderResult {
    let transport = transport_for_status(http_status);
    match http_status {
        429 => {
            return ProviderResult::temporarily_unavailable(
                ReasonCode::ProviderRateLimited,
                transport,
            )
        }
        500..=599 => {
            return ProviderResult::temporarily_unavailable(
                ReasonCode::ProviderServerError,
                transport,
            )
        }
        200..=299 | 400..=499 => {}
        _ => {
            return ProviderResult::temporarily_unavailable(
                ReasonCode::MalformedResponse,
                transport,
            )
        }
    }
    let malformed = ProviderResult::temporarily_unavailable(
        ReasonCode::MalformedResponse,
        if (200..=299).contains(&http_status) {
            TransportClass::MalformedBody
        } else {
            transport
        },
    );
    let Ok(parsed) = serde_json::from_slice::<SiteverifyBody>(body) else {
        return malformed;
    };
    let Some(success) = parsed.success else {
        return malformed;
    };

    if !success {
        let has = |code: &str| parsed.error_codes.iter().any(|c| c == code);
        if has("missing-input-secret") {
            return ProviderResult::configuration_invalid(ReasonCode::MissingSecret, transport);
        }
        if has("invalid-input-secret") {
            return ProviderResult::configuration_invalid(ReasonCode::InvalidSecret, transport);
        }
        if has("internal-error") {
            return ProviderResult::temporarily_unavailable(
                ReasonCode::ProviderServerError,
                transport,
            );
        }
        if has("timeout-or-duplicate") {
            return ProviderResult::attempt_rejected(ReasonCode::TokenDuplicate, transport);
        }
        if has("invalid-input-response") || has("missing-input-response") || has("bad-request") {
            return ProviderResult::attempt_rejected(ReasonCode::TokenRejected, transport);
        }
        return malformed;
    }

    if !(200..=299).contains(&http_status) {
        return malformed;
    }
    let hostname_ok = parsed
        .hostname
        .as_deref()
        .is_some_and(|h| h.eq_ignore_ascii_case(&expected.hostname));
    if !hostname_ok {
        return ProviderResult::attempt_rejected(ReasonCode::HostnameMismatch, transport);
    }
    if let Some(action) = &expected.action {
        if parsed.action.as_deref() != Some(action.as_str()) {
            return ProviderResult::attempt_rejected(ReasonCode::ActionMismatch, transport);
        }
    }
    let issued_at = parsed
        .challenge_ts
        .as_deref()
        .and_then(|ts| OffsetDateTime::parse(ts, &Rfc3339).ok());
    match issued_at {
        Some(issued_at)
            if now - issued_at <= expected.token_max_age
                && issued_at - now <= time::Duration::seconds(60) =>
        {
            ProviderResult::business_success(transport)
        }
        _ => ProviderResult::attempt_rejected(ReasonCode::TokenExpired, transport),
    }
}

/// 負の probe の結果を readiness 用に写像する。provider が secret を受理して応答した
/// (token 起因の拒否・または成功) なら `Accepted`。business success にはしない。
fn probe_outcome(result: ProviderResult) -> ProviderResult {
    use kernel::prelude::entity::ProviderOutcome::*;
    match result.outcome {
        BusinessSuccess | AttemptRejected | Accepted => ProviderResult::accepted(result.transport),
        ConfigurationInvalid | TemporarilyUnavailable => result,
    }
}

#[derive(Clone)]
pub struct HttpTurnstileVerifier {
    settings: Arc<TurnstileSettings>,
    network: TurnstileNetworkSettings,
    base_url: String,
    allow_insecure: bool,
}

impl std::fmt::Debug for HttpTurnstileVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpTurnstileVerifier")
            .field("settings", &self.settings)
            .finish()
    }
}

impl HttpTurnstileVerifier {
    pub fn new(settings: TurnstileSettings) -> Self {
        Self {
            settings: Arc::new(settings),
            network: TurnstileNetworkSettings::default(),
            base_url: SITEVERIFY_URL.to_string(),
            allow_insecure: false,
        }
    }

    pub fn from_env() -> Self {
        Self::new(TurnstileSettings::from_env())
    }

    #[cfg(test)]
    pub(crate) fn base_url_for_test(&self) -> &str {
        &self.base_url
    }

    #[cfg(test)]
    pub(crate) fn allows_insecure_for_test(&self) -> bool {
        self.allow_insecure
    }

    pub fn with_network_settings(mut self, network: TurnstileNetworkSettings) -> Self {
        self.network = network;
        self
    }

    /// Siteverify の URL を差し替える (fake provider / wiremock 用)。`cfg(test)` / `test-mode`
    /// ビルドにだけ存在し、環境変数からは到達できない。http も許可される。
    #[cfg(any(test, feature = "test-mode"))]
    pub fn with_test_base_url(mut self, url: &str) -> Self {
        self.base_url = url.to_string();
        self.allow_insecure = true;
        self
    }

    fn client(&self) -> Result<reqwest::Client, ProviderResult> {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .connect_timeout(self.network.connect_timeout)
            .timeout(self.network.total_timeout)
            .pool_max_idle_per_host(0)
            .https_only(!self.allow_insecure)
            .build()
            .map_err(|_| {
                ProviderResult::configuration_invalid(
                    ReasonCode::InvalidConfiguration,
                    TransportClass::None,
                )
            })
    }

    async fn call(&self, token: &str, remote_ip: Option<&str>) -> ProviderResult {
        let snapshot = &self.settings.snapshot;
        if !snapshot.is_usable() {
            return ProviderResult::configuration_invalid(
                snapshot.issue.unwrap_or(ReasonCode::NotConfigured),
                TransportClass::None,
            );
        }
        let Some(secret) = &self.settings.secret else {
            return ProviderResult::configuration_invalid(
                ReasonCode::MissingSecret,
                TransportClass::None,
            );
        };
        let client = match self.client() {
            Ok(client) => client,
            Err(result) => return result,
        };
        let mut form: Vec<(&str, &str)> = vec![("secret", secret.as_str()), ("response", token)];
        if let Some(ip) = remote_ip {
            form.push(("remoteip", ip));
        }
        let response = client.post(&self.base_url).form(&form).send().await;
        let mut response = match response {
            Ok(response) => response,
            Err(error) => return classify_transport_error(&error),
        };
        let status = response.status().as_u16();
        let mut body: Vec<u8> = Vec::new();
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    if body.len() + chunk.len() > self.network.max_response_bytes {
                        return ProviderResult::temporarily_unavailable(
                            ReasonCode::MalformedResponse,
                            TransportClass::MalformedBody,
                        );
                    }
                    body.extend_from_slice(&chunk);
                }
                Ok(None) => break,
                Err(error) => return classify_transport_error(&error),
            }
        }
        classify_siteverify(
            status,
            &body,
            &self.settings.expectation,
            OffsetDateTime::now_utc(),
        )
    }
}

fn classify_transport_error(error: &reqwest::Error) -> ProviderResult {
    if error.is_timeout() {
        return ProviderResult::temporarily_unavailable(
            ReasonCode::Timeout,
            TransportClass::Timeout,
        );
    }
    let mut chain = String::new();
    let mut source: Option<&dyn std::error::Error> = Some(error);
    while let Some(current) = source {
        chain.push_str(&current.to_string().to_ascii_lowercase());
        chain.push(' ');
        source = current.source();
    }
    if chain.contains("certificate") || chain.contains("tls") || chain.contains("ssl") {
        return ProviderResult::temporarily_unavailable(
            ReasonCode::TlsFailed,
            TransportClass::TlsFailed,
        );
    }
    ProviderResult::temporarily_unavailable(
        ReasonCode::ConnectFailed,
        TransportClass::ConnectFailed,
    )
}

impl TurnstileVerifier for HttpTurnstileVerifier {
    fn snapshot(&self) -> ProtectionConfigSnapshot {
        self.settings.snapshot.clone()
    }

    async fn verify(&self, check: &BotTokenCheck<'_>) -> ProviderResult {
        let token = check.token.trim();
        if !self.settings.snapshot.is_usable() {
            return self.call(token, None).await;
        }
        if token.is_empty() || token.len() > MAX_TOKEN_LEN {
            return ProviderResult::attempt_rejected(
                ReasonCode::TokenRejected,
                TransportClass::None,
            );
        }
        self.call(token, check.remote_ip).await
    }

    async fn probe_secret(&self) -> ProviderResult {
        probe_outcome(self.call(PROBE_TOKEN, None).await)
    }
}
