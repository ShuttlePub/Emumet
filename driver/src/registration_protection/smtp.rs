//! SMTP adapter (lettre)。
//!
//! 実際に使う SMTP 設定 (host/port/TLS/認証/from) から設定世代を導き、code メールの送信と
//! 接続・認証 probe を提供する。SMTP の 2xx は「受理」(`Accepted`) であって所有確認ではない。
//! 失敗は ADR の分類に写像する: 認証・設定不正・TLS 不正 = `configuration_invalid`、宛先拒否
//! (550 等) = `attempt_rejected`、接続失敗・timeout・4xx = `temporarily_unavailable`。
//! code・本文・secret はログ・エラー・分類に出さない。

use super::config::{
    derive_generation, explicit_enabled, load_secret, non_empty, secret_fingerprint, Secret,
    SecretError, ValueSource,
};
use kernel::interfaces::repository::EmailTransport;
use kernel::prelude::entity::{
    NormalizedEmail, Protection, ProtectionConfigSnapshot, ProtectionProvider, ProviderResult,
    ReasonCode, TransportClass,
};
use lettre::message::header::ContentType;
use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::Tls;
use lettre::transport::smtp::client::TlsParameters;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use std::str::FromStr;
use std::time::Duration;

pub const ENV_ENABLED: &str = "REGISTRATION_EMAIL_ENABLED";
pub const ENV_HOST: &str = "REGISTRATION_SMTP_HOST";
pub const ENV_PORT: &str = "REGISTRATION_SMTP_PORT";
pub const ENV_TLS: &str = "REGISTRATION_SMTP_TLS";
pub const ENV_USERNAME: &str = "REGISTRATION_SMTP_USERNAME";
pub const ENV_PASSWORD: &str = "REGISTRATION_SMTP_PASSWORD";
pub const ENV_PASSWORD_FILE: &str = "REGISTRATION_SMTP_PASSWORD_FILE";
pub const ENV_FROM: &str = "REGISTRATION_EMAIL_FROM";
pub const ENV_CANARY: &str = "REGISTRATION_EMAIL_CANARY_ADDRESS";

const SMTP_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmtpTlsMode {
    /// STARTTLS を必須にする (既定。submission 587)。
    StartTls,
    /// 接続直後から TLS (submissions 465)。
    Tls,
    /// 平文。ローカル SMTP 模擬 (テスト) 専用で、`cfg(test)` / `test-mode` ビルドにだけ存在する。
    #[cfg(any(test, feature = "test-mode"))]
    Plaintext,
}

impl SmtpTlsMode {
    fn as_str(&self) -> &'static str {
        match self {
            Self::StartTls => "starttls",
            Self::Tls => "tls",
            #[cfg(any(test, feature = "test-mode"))]
            Self::Plaintext => "plaintext",
        }
    }

    fn default_port(&self) -> u16 {
        match self {
            Self::StartTls => 587,
            Self::Tls => 465,
            #[cfg(any(test, feature = "test-mode"))]
            Self::Plaintext => 25,
        }
    }
}

/// 読み込んだ SMTP 設定 (secret を含むので Debug は伏せる)。
pub struct SmtpSettings {
    snapshot: ProtectionConfigSnapshot,
    transport: Option<AsyncSmtpTransport<Tokio1Executor>>,
    from: Option<Mailbox>,
    canary: Option<NormalizedEmail>,
    /// 1 回の送信・probe 全体の期限。lettre の timeout は応答待ちに効かない場面があるため、
    /// 無応答の SMTP サーバーで公開 API が固まらないよう全体にも期限を掛ける。
    deadline: Duration,
}

impl std::fmt::Debug for SmtpSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmtpSettings")
            .field("enabled", &self.snapshot.enabled)
            .field("issue", &self.snapshot.issue)
            .field("generation", &self.snapshot.generation)
            .finish()
    }
}

fn parse_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b':')
}

impl SmtpSettings {
    pub fn from_env() -> Self {
        Self::from_source(&super::config::ProcessEnv)
    }

    pub fn from_source(source: &dyn ValueSource) -> Self {
        Self::build(source, false, SMTP_TIMEOUT)
    }

    /// 平文 SMTP (`REGISTRATION_SMTP_TLS=plaintext`) を許可する構築。ローカル SMTP 模擬用で、
    /// `cfg(test)` / `test-mode` ビルドにだけ存在する。
    #[cfg(any(test, feature = "test-mode"))]
    pub fn from_source_allowing_plaintext(source: &dyn ValueSource) -> Self {
        Self::build(source, true, SMTP_TIMEOUT)
    }

    /// `from_source_allowing_plaintext` に SMTP の timeout を指定する (timeout の試験用)。
    #[cfg(test)]
    pub(crate) fn from_source_allowing_plaintext_with_timeout(
        source: &dyn ValueSource,
        timeout: Duration,
    ) -> Self {
        Self::build(source, true, timeout)
    }

    fn build(source: &dyn ValueSource, allow_plaintext: bool, timeout: Duration) -> Self {
        let host = non_empty(source, ENV_HOST);
        let port_raw = non_empty(source, ENV_PORT);
        let tls_raw = non_empty(source, ENV_TLS);
        let username = non_empty(source, ENV_USERNAME);
        let from_raw = non_empty(source, ENV_FROM);
        let canary_raw = non_empty(source, ENV_CANARY);
        let explicit = explicit_enabled(source, ENV_ENABLED);
        let password = load_secret(source, ENV_PASSWORD, ENV_PASSWORD_FILE);

        let any_present = host.is_some()
            || port_raw.is_some()
            || tls_raw.is_some()
            || username.is_some()
            || from_raw.is_some()
            || canary_raw.is_some()
            || non_empty(source, ENV_PASSWORD).is_some()
            || non_empty(source, ENV_PASSWORD_FILE).is_some();

        let (enabled, mut issue) = match explicit {
            Some(false) => (false, Some(ReasonCode::Disabled)),
            _ if !any_present => (false, Some(ReasonCode::NotConfigured)),
            _ => (true, None),
        };

        let tls_mode = match tls_raw.as_deref().map(str::to_ascii_lowercase).as_deref() {
            None | Some("starttls") => Some(SmtpTlsMode::StartTls),
            Some("tls") => Some(SmtpTlsMode::Tls),
            #[cfg(any(test, feature = "test-mode"))]
            Some("plaintext") if allow_plaintext => Some(SmtpTlsMode::Plaintext),
            _ => None,
        };
        let _ = allow_plaintext;
        let password: Option<Secret> = match password {
            Ok(password) => password,
            Err(SecretError::Ambiguous) => {
                issue = issue.or(Some(ReasonCode::InvalidConfiguration));
                None
            }
            Err(SecretError::Unreadable) => {
                issue = issue.or(Some(ReasonCode::MissingSecret));
                None
            }
        };
        let port = match (&port_raw, tls_mode) {
            (Some(raw), _) => raw.parse::<u16>().ok().filter(|p| *p != 0),
            (None, Some(mode)) => Some(mode.default_port()),
            (None, None) => None,
        };
        let from = from_raw
            .as_deref()
            .and_then(|raw| Mailbox::from_str(raw).ok());
        let canary = match canary_raw.as_deref() {
            None => None,
            Some(raw) => NormalizedEmail::parse(raw).ok(),
        };

        if enabled && issue.is_none() {
            issue = match (&host, &username, &password) {
                _ if !host.as_deref().is_some_and(parse_host) => {
                    Some(ReasonCode::InvalidConfiguration)
                }
                _ if tls_mode.is_none() || port.is_none() => Some(ReasonCode::InvalidConfiguration),
                _ if from.is_none() => Some(ReasonCode::InvalidConfiguration),
                _ if canary_raw.is_some() && canary.is_none() => {
                    Some(ReasonCode::InvalidConfiguration)
                }
                (_, Some(_), None) => Some(ReasonCode::MissingSecret),
                (_, None, Some(_)) => Some(ReasonCode::InvalidConfiguration),
                _ => None,
            };
        }

        let fingerprint = password
            .as_ref()
            .map(|secret| secret_fingerprint(secret.as_bytes()))
            .unwrap_or_default();
        let port_text = port.map(|p| p.to_string()).unwrap_or_default();
        let generation = derive_generation(
            "email",
            ProtectionProvider::Smtp.as_str(),
            enabled,
            &[
                ("host", host.as_deref().unwrap_or("")),
                ("port", &port_text),
                ("tls", tls_mode.map(|m| m.as_str()).unwrap_or("")),
                ("username", username.as_deref().unwrap_or("")),
                ("from", from_raw.as_deref().unwrap_or("")),
                ("canary", canary_raw.as_deref().unwrap_or("")),
            ],
            &fingerprint,
        );

        let mut transport = None;
        if enabled && issue.is_none() {
            match build_transport(
                host.as_deref().unwrap_or_default(),
                port.unwrap_or(587),
                tls_mode.unwrap_or(SmtpTlsMode::StartTls),
                username.as_deref(),
                password.as_ref(),
                timeout,
            ) {
                Ok(built) => transport = Some(built),
                Err(reason) => issue = Some(reason),
            }
        }

        Self {
            snapshot: ProtectionConfigSnapshot {
                protection: Protection::Email,
                provider: ProtectionProvider::Smtp,
                enabled,
                issue,
                generation,
                fingerprint,
            },
            transport,
            from,
            canary,
            deadline: timeout,
        }
    }

    pub fn snapshot(&self) -> &ProtectionConfigSnapshot {
        &self.snapshot
    }
}

fn build_transport(
    host: &str,
    port: u16,
    tls: SmtpTlsMode,
    username: Option<&str>,
    password: Option<&Secret>,
    timeout: Duration,
) -> Result<AsyncSmtpTransport<Tokio1Executor>, ReasonCode> {
    let tls_for =
        |host: &str| TlsParameters::new(host.to_string()).map_err(|_| ReasonCode::SmtpTlsFailure);
    let mut builder = match tls {
        SmtpTlsMode::StartTls => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host)
            .tls(Tls::Required(tls_for(host)?)),
        SmtpTlsMode::Tls => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host)
            .tls(Tls::Wrapper(tls_for(host)?)),
        #[cfg(any(test, feature = "test-mode"))]
        SmtpTlsMode::Plaintext => {
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host).tls(Tls::None)
        }
    };
    builder = builder.port(port).timeout(Some(timeout));
    if let (Some(username), Some(password)) = (username, password) {
        builder = builder.credentials(Credentials::new(
            username.to_string(),
            password.as_str().to_string(),
        ));
    }
    Ok(builder.build())
}

/// SMTP 失敗の観測 (lettre のエラーから導く。分類の純粋関数の入力)。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SmtpFailure {
    pub code: Option<u16>,
    pub timeout: bool,
    pub tls: bool,
    pub client: bool,
}

/// SMTP 失敗を分類する純粋関数。`recipient_phase` は宛先を伴う送信 (true) か、接続・認証だけの
/// probe (false)。probe に宛先拒否はないので、55x でも `attempt_rejected` にしない。
pub fn classify_smtp_failure(failure: SmtpFailure, recipient_phase: bool) -> ProviderResult {
    if failure.timeout {
        return ProviderResult::temporarily_unavailable(
            ReasonCode::Timeout,
            TransportClass::Timeout,
        );
    }
    if failure.tls {
        return ProviderResult::configuration_invalid(
            ReasonCode::SmtpTlsFailure,
            TransportClass::TlsFailed,
        );
    }
    match failure.code {
        Some(400..=499) => ProviderResult::temporarily_unavailable(
            ReasonCode::SmtpTransientFailure,
            TransportClass::SmtpTransient,
        ),
        Some(530 | 534 | 535 | 538) => ProviderResult::configuration_invalid(
            ReasonCode::SmtpAuthFailed,
            TransportClass::SmtpAuthRejected,
        ),
        Some(550..=554) if recipient_phase => ProviderResult::attempt_rejected(
            ReasonCode::RecipientRejected,
            TransportClass::SmtpPermanent,
        ),
        Some(500..=599) => ProviderResult::configuration_invalid(
            ReasonCode::SmtpPermanentFailure,
            TransportClass::SmtpPermanent,
        ),
        Some(_) => ProviderResult::temporarily_unavailable(
            ReasonCode::SmtpTransientFailure,
            TransportClass::SmtpTransient,
        ),
        None if failure.client => ProviderResult::configuration_invalid(
            ReasonCode::InvalidConfiguration,
            TransportClass::None,
        ),
        None => ProviderResult::temporarily_unavailable(
            ReasonCode::ConnectFailed,
            TransportClass::ConnectFailed,
        ),
    }
}

fn failure_of(error: &lettre::transport::smtp::Error) -> SmtpFailure {
    use std::fmt::Write;
    let code = error.status().map(|code| {
        let mut digits = String::new();
        let _ = write!(digits, "{code}");
        digits.parse::<u16>().unwrap_or(0)
    });
    SmtpFailure {
        code,
        timeout: error.is_timeout(),
        tls: error.is_tls(),
        client: error.is_client(),
    }
}

#[derive(Clone)]
pub struct SmtpEmailTransport {
    settings: std::sync::Arc<SmtpSettings>,
}

impl std::fmt::Debug for SmtpEmailTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmtpEmailTransport")
            .field("settings", &self.settings)
            .finish()
    }
}

impl SmtpEmailTransport {
    pub fn new(settings: SmtpSettings) -> Self {
        Self {
            settings: std::sync::Arc::new(settings),
        }
    }

    pub fn from_env() -> Self {
        Self::new(SmtpSettings::from_env())
    }

    fn unusable(&self) -> Option<ProviderResult> {
        let snapshot = &self.settings.snapshot;
        if snapshot.is_usable() && self.settings.transport.is_some() {
            return None;
        }
        Some(ProviderResult::configuration_invalid(
            snapshot.issue.unwrap_or(ReasonCode::NotConfigured),
            TransportClass::None,
        ))
    }
}

impl EmailTransport for SmtpEmailTransport {
    fn snapshot(&self) -> ProtectionConfigSnapshot {
        self.settings.snapshot.clone()
    }

    fn canary_address(&self) -> Option<String> {
        self.settings
            .canary
            .as_ref()
            .map(|c| c.as_str().to_string())
    }

    async fn send_code(&self, to: &str, code: &str) -> ProviderResult {
        if let Some(result) = self.unusable() {
            return result;
        }
        let (Some(transport), Some(from)) = (&self.settings.transport, &self.settings.from) else {
            return ProviderResult::configuration_invalid(
                ReasonCode::InvalidConfiguration,
                TransportClass::None,
            );
        };
        let recipient = match NormalizedEmail::parse(to)
            .ok()
            .and_then(|email| Mailbox::from_str(email.as_str()).ok())
        {
            Some(recipient) => recipient,
            None => {
                return ProviderResult::attempt_rejected(
                    ReasonCode::RecipientRejected,
                    TransportClass::None,
                )
            }
        };
        let message = Message::builder()
            .from(from.clone())
            .to(recipient)
            .subject("Emumet verification code")
            .header(ContentType::TEXT_PLAIN)
            .body(format!(
                "Your Emumet verification code is: {code}\n\n\
                 It expires in a few minutes. If you did not request it, ignore this message.\n"
            ));
        let Ok(message) = message else {
            return ProviderResult::configuration_invalid(
                ReasonCode::InvalidConfiguration,
                TransportClass::None,
            );
        };
        let sent = tokio::time::timeout(self.settings.deadline, transport.send(message)).await;
        let Ok(sent) = sent else {
            return ProviderResult::temporarily_unavailable(
                ReasonCode::Timeout,
                TransportClass::Timeout,
            );
        };
        match sent {
            Ok(response) if response.is_positive() => {
                ProviderResult::accepted(TransportClass::SmtpAccepted)
            }
            Ok(_) => ProviderResult::temporarily_unavailable(
                ReasonCode::SmtpTransientFailure,
                TransportClass::SmtpTransient,
            ),
            Err(error) => classify_smtp_failure(failure_of(&error), true),
        }
    }

    async fn probe_connection(&self) -> ProviderResult {
        if let Some(result) = self.unusable() {
            return result;
        }
        let Some(transport) = &self.settings.transport else {
            return ProviderResult::configuration_invalid(
                ReasonCode::InvalidConfiguration,
                TransportClass::None,
            );
        };
        let probed =
            tokio::time::timeout(self.settings.deadline, transport.test_connection()).await;
        let Ok(probed) = probed else {
            return ProviderResult::temporarily_unavailable(
                ReasonCode::Timeout,
                TransportClass::Timeout,
            );
        };
        match probed {
            Ok(true) => ProviderResult::accepted(TransportClass::SmtpAccepted),
            Ok(false) => ProviderResult::temporarily_unavailable(
                ReasonCode::ConnectFailed,
                TransportClass::ConnectFailed,
            ),
            Err(error) => classify_smtp_failure(failure_of(&error), false),
        }
    }
}
