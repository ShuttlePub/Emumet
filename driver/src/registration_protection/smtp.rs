//! SMTP adapter (lettre)。
//!
//! 実際に使う SMTP 設定 (host/port/TLS/認証/from) から設定世代を導き、code メールの送信と
//! 接続・認証・MAIL FROM の probe を提供する。SMTP の 2xx は「受理」(`Accepted`) であって所有確認ではない。
//! 失敗は ADR の分類に写像する: 認証・設定不正・TLS 不正 = `configuration_invalid`、宛先拒否
//! (550 等) = `attempt_rejected`、接続失敗・timeout・4xx = `temporarily_unavailable`。
//! code・本文・secret はログ・エラー・分類に出さない。

use super::config::{
    derive_generation, explicit_enabled, load_secret, non_empty, secret_fingerprint, Secret,
    SecretError, ValueSource,
};
use kernel::interfaces::repository::{EmailTransport, SendPurpose};
use kernel::prelude::entity::{
    CheckStage, NormalizedEmail, Protection, ProtectionConfigSnapshot, ProtectionProvider,
    ProviderOutcome, ProviderResult, ReasonCode, TransportClass,
};
use lettre::address::Envelope;
use lettre::message::header::ContentType;
use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::authentication::Mechanism;
use lettre::transport::smtp::client::{AsyncSmtpConnection, TlsParameters};
use lettre::transport::smtp::commands::{Data, Mail, Rcpt, Rset};
use lettre::transport::smtp::extension::ClientId;
use lettre::Address;
use lettre::Message;
use std::str::FromStr;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;
use time::OffsetDateTime;

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
/// QUIT の応答待ち。確定済みの結果 (DATA 受理など) を QUIT の無応答で覆さないため、全体期限とは別に短く掛ける。
const QUIT_TIMEOUT: Duration = Duration::from_secs(2);

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
    session: Option<SmtpSession>,
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

        let mut session = None;
        if enabled && issue.is_none() {
            match build_session(
                host.as_deref().unwrap_or_default(),
                port.unwrap_or(587),
                tls_mode.unwrap_or(SmtpTlsMode::StartTls),
                username.as_deref(),
                password.as_ref(),
                timeout,
            ) {
                Ok(built) => session = Some(built),
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
            session,
            from,
            canary,
            deadline: timeout,
        }
    }

    pub fn snapshot(&self) -> &ProtectionConfigSnapshot {
        &self.snapshot
    }
}

/// SMTP の 1 セッションの接続設定 (secret を含むので Debug を実装しない)。
struct SmtpSession {
    host: String,
    port: u16,
    mode: SmtpTlsMode,
    tls: Option<TlsParameters>,
    credentials: Option<Credentials>,
    timeout: Duration,
}

fn build_session(
    host: &str,
    port: u16,
    mode: SmtpTlsMode,
    username: Option<&str>,
    password: Option<&Secret>,
    timeout: Duration,
) -> Result<SmtpSession, ReasonCode> {
    let tls = match mode {
        SmtpTlsMode::StartTls | SmtpTlsMode::Tls => {
            Some(TlsParameters::new(host.to_string()).map_err(|_| ReasonCode::SmtpTlsFailure)?)
        }
        #[cfg(any(test, feature = "test-mode"))]
        SmtpTlsMode::Plaintext => None,
    };
    let credentials = match (username, password) {
        (Some(username), Some(password)) => Some(Credentials::new(
            username.to_string(),
            password.as_str().to_string(),
        )),
        _ => None,
    };
    Ok(SmtpSession {
        host: host.to_string(),
        port,
        mode,
        tls,
        credentials,
        timeout,
    })
}

/// 失敗した SMTP の段階。宛先拒否 (RCPT) と送信者・認証・transport の設定不正を分けるために保持する。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SmtpStage {
    /// 接続・EHLO。
    #[default]
    Connect,
    StartTls,
    Auth,
    MailFrom,
    Rcpt,
    Data,
}

impl SmtpStage {
    fn as_u8(self) -> u8 {
        match self {
            Self::Connect => 0,
            Self::StartTls => 1,
            Self::Auth => 2,
            Self::MailFrom => 3,
            Self::Rcpt => 4,
            Self::Data => 5,
        }
    }

    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::StartTls,
            2 => Self::Auth,
            3 => Self::MailFrom,
            4 => Self::Rcpt,
            5 => Self::Data,
            _ => Self::Connect,
        }
    }
}

impl From<SmtpStage> for CheckStage {
    fn from(stage: SmtpStage) -> Self {
        match stage {
            SmtpStage::Connect => Self::Connect,
            SmtpStage::StartTls => Self::StartTls,
            SmtpStage::Auth => Self::Auth,
            SmtpStage::MailFrom => Self::MailFrom,
            SmtpStage::Rcpt => Self::Rcpt,
            SmtpStage::Data => Self::Data,
        }
    }
}

/// SMTP 失敗の観測 (分類の純粋関数の入力)。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SmtpFailure {
    pub stage: SmtpStage,
    pub code: Option<u16>,
    pub timeout: bool,
    pub tls: bool,
    pub client: bool,
}

/// SMTP 失敗を段階と送信の目的つきで分類する純粋関数。
///
/// **公開要求 (`SendPurpose::Registration`)**:
/// - **宛先を提示した後 (RCPT コマンド送信以降: RCPT・DATA の全コード、その段階の timeout・切断) の失敗は
///   すべて `attempt_rejected`**。応答コードや段階だけでは宛先非依存と保証できず、readiness を変えると
///   先行する宛先の状態が後続の公開応答 (202/503) から判別できてしまうため (非列挙契約)。RCPT の 550-554 は
///   `RecipientRejected`、それ以外は `RecipientDeferred`
/// - 宛先提示前 (接続・STARTTLS・認証・MAIL FROM) の失敗だけが provider の障害を示せる: timeout・接続失敗・
///   4xx は一時障害、TLS 不正・認証失敗 (530/534/535/538)・STARTTLS 非対応・5xx は `configuration_invalid`
/// - provider の健全性は宛先を伴わない独立 probe と管理者の canary で判定する
///
/// **管理者 canary (`SendPurpose::Canary`)**: 宛先は固定の承認済み mailbox で攻撃者が選べないので、RCPT・DATA・
/// timeout・切断を含むすべての段階の失敗を provider の障害として分類する (4xx・timeout・切断は一時障害、
/// 認証失敗・5xx・設定不備は設定不正。canary 宛先の 550 系も設定不正)
pub fn classify_smtp_failure(failure: SmtpFailure, purpose: SendPurpose) -> ProviderResult {
    let result = classify_outcome(failure, purpose);
    // 障害を解除する証拠の段階を失敗した段階で記録する (`attempt_rejected` は記録されない)。
    if result.outcome == ProviderOutcome::AttemptRejected {
        result
    } else {
        result.at_stage(failure.stage.into())
    }
}

fn classify_outcome(failure: SmtpFailure, purpose: SendPurpose) -> ProviderResult {
    if purpose == SendPurpose::Registration
        && matches!(failure.stage, SmtpStage::Rcpt | SmtpStage::Data)
    {
        let transport = match failure.code {
            _ if failure.timeout => TransportClass::Timeout,
            Some(500..=599) => TransportClass::SmtpPermanent,
            _ => TransportClass::SmtpTransient,
        };
        let reason = match failure.code {
            Some(550..=554) if failure.stage == SmtpStage::Rcpt => ReasonCode::RecipientRejected,
            _ => ReasonCode::RecipientDeferred,
        };
        return ProviderResult::attempt_rejected(reason, transport);
    }
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

fn failure_of(error: &lettre::transport::smtp::Error, stage: SmtpStage) -> SmtpFailure {
    use std::fmt::Write;
    let code = error.status().map(|code| {
        let mut digits = String::new();
        let _ = write!(digits, "{code}");
        digits.parse::<u16>().unwrap_or(0)
    });
    SmtpFailure {
        stage,
        code,
        timeout: error.is_timeout(),
        tls: error.is_tls(),
        client: error.is_client(),
    }
}

/// 段階つきの生の失敗。分類 (観測への反映可否) は送信の目的が決まる呼出し側で行う。
type StageResult<T> = Result<T, SmtpFailure>;

fn at_stage<T>(
    result: Result<T, lettre::transport::smtp::Error>,
    stage: SmtpStage,
) -> StageResult<T> {
    result.map_err(|error| failure_of(&error, stage))
}

impl SmtpSession {
    /// 接続 → (STARTTLS) → 認証。ここまでが「接続・認証 probe」。
    async fn open(&self, progress: &AtomicU8) -> StageResult<AsyncSmtpConnection> {
        progress.store(SmtpStage::Connect.as_u8(), Ordering::SeqCst);
        let hello = ClientId::default();
        let wrapper = match self.mode {
            SmtpTlsMode::Tls => self.tls.clone(),
            _ => None,
        };
        let mut connection = at_stage(
            AsyncSmtpConnection::connect_tokio1(
                (self.host.as_str(), self.port),
                Some(self.timeout),
                &hello,
                wrapper,
                None,
            )
            .await,
            SmtpStage::Connect,
        )?;
        if self.mode == SmtpTlsMode::StartTls {
            progress.store(SmtpStage::StartTls.as_u8(), Ordering::SeqCst);
            let Some(tls) = self.tls.clone() else {
                return Err(SmtpFailure {
                    stage: SmtpStage::StartTls,
                    client: true,
                    ..SmtpFailure::default()
                });
            };
            // STARTTLS を必須にする。サーバーが提供しなければ平文へ降格せずエラー (client error)。
            at_stage(connection.starttls(tls, &hello).await, SmtpStage::StartTls)?;
        }
        if let Some(credentials) = &self.credentials {
            progress.store(SmtpStage::Auth.as_u8(), Ordering::SeqCst);
            at_stage(
                connection
                    .auth(&[Mechanism::Plain, Mechanism::Login], credentials)
                    .await,
                SmtpStage::Auth,
            )?;
        }
        Ok(connection)
    }

    /// 全体の期限 (`deadline`) は接続から DATA 受理までに掛け、QUIT は含めない。
    /// 成功は **DATA を受理された時刻** を返す (その時刻が配送成功の証拠時刻)。
    async fn send(
        &self,
        envelope: &Envelope,
        body: &[u8],
        deadline: Duration,
    ) -> StageResult<OffsetDateTime> {
        // 最後に開始した段階。全体期限が切れたとき、宛先を提示した後 (RCPT 以降) の timeout は宛先に
        // 依存し得るので、段階つきで分類する。
        let progress = AtomicU8::new(SmtpStage::Connect.as_u8());
        let work = async {
            let mut connection = self.open(&progress).await?;
            let result = Self::deliver(&mut connection, envelope, body, &progress).await;
            Ok::<_, SmtpFailure>((result, connection))
        };
        match tokio::time::timeout(deadline, work).await {
            Err(_) => Err(SmtpFailure {
                stage: SmtpStage::from_u8(progress.load(Ordering::SeqCst)),
                timeout: true,
                ..SmtpFailure::default()
            }),
            Ok(Err(result)) => Err(result),
            Ok(Ok((result, mut connection))) => {
                // 切断は best effort。確定済みの結果は QUIT の成否・遅延に影響されない。
                let _ = tokio::time::timeout(QUIT_TIMEOUT, connection.quit()).await;
                result
            }
        }
    }

    async fn deliver(
        connection: &mut AsyncSmtpConnection,
        envelope: &Envelope,
        body: &[u8],
        progress: &AtomicU8,
    ) -> StageResult<OffsetDateTime> {
        progress.store(SmtpStage::MailFrom.as_u8(), Ordering::SeqCst);
        at_stage(
            connection
                .command(Mail::new(envelope.from().cloned(), vec![]))
                .await,
            SmtpStage::MailFrom,
        )?;
        // RCPT コマンドを送った時点で宛先を提示したことになる。
        progress.store(SmtpStage::Rcpt.as_u8(), Ordering::SeqCst);
        for recipient in envelope.to() {
            at_stage(
                connection
                    .command(Rcpt::new(recipient.clone(), vec![]))
                    .await,
                SmtpStage::Rcpt,
            )?;
        }
        progress.store(SmtpStage::Data.as_u8(), Ordering::SeqCst);
        at_stage(connection.command(Data).await, SmtpStage::Data)?;
        at_stage(connection.message(body).await, SmtpStage::Data)?;
        Ok(OffsetDateTime::now_utc())
    }

    /// 接続 → (STARTTLS) → 認証 → MAIL FROM (宛先は提示しない) → RSET。成功は **MAIL FROM を受理された時刻** を
    /// 返す。MAIL FROM までの段階の障害を覆えるが、RCPT・DATA の障害は覆えない。
    async fn probe(&self, from: Address, deadline: Duration) -> StageResult<OffsetDateTime> {
        let progress = AtomicU8::new(SmtpStage::Connect.as_u8());
        let work = async {
            let mut connection = self.open(&progress).await?;
            progress.store(SmtpStage::MailFrom.as_u8(), Ordering::SeqCst);
            let accepted = at_stage(
                connection.command(Mail::new(Some(from), vec![])).await,
                SmtpStage::MailFrom,
            );
            let at = OffsetDateTime::now_utc();
            if accepted.is_ok() {
                // 取引は破棄する (メールは送らない)。RSET の成否は結果に影響しない。
                let _ = tokio::time::timeout(QUIT_TIMEOUT, connection.command(Rset)).await;
            }
            Ok::<_, SmtpFailure>((accepted.map(|_| at), connection))
        };
        let (accepted, mut connection) = match tokio::time::timeout(deadline, work).await {
            Err(_) => {
                return Err(SmtpFailure {
                    stage: SmtpStage::from_u8(progress.load(Ordering::SeqCst)),
                    timeout: true,
                    ..SmtpFailure::default()
                })
            }
            Ok(Err(result)) => return Err(result),
            Ok(Ok(done)) => done,
        };
        let _ = tokio::time::timeout(QUIT_TIMEOUT, connection.quit()).await;
        accepted
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
        if snapshot.is_usable() && self.settings.session.is_some() {
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

    async fn send_code(&self, to: &str, code: &str, purpose: SendPurpose) -> ProviderResult {
        if let Some(result) = self.unusable() {
            return result;
        }
        let (Some(session), Some(from)) = (&self.settings.session, &self.settings.from) else {
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
        let body = message.formatted();
        match session
            .send(message.envelope(), &body, self.settings.deadline)
            .await
        {
            Ok(at) => ProviderResult::accepted(TransportClass::SmtpAccepted)
                .at_stage(CheckStage::Data)
                .observed(at),
            Err(failure) => {
                classify_smtp_failure(failure, purpose).observed(OffsetDateTime::now_utc())
            }
        }
    }

    async fn probe_connection(&self) -> ProviderResult {
        if let Some(result) = self.unusable() {
            return result;
        }
        let (Some(session), Some(from)) = (&self.settings.session, &self.settings.from) else {
            return ProviderResult::configuration_invalid(
                ReasonCode::InvalidConfiguration,
                TransportClass::None,
            );
        };
        match session
            .probe(from.email.clone(), self.settings.deadline)
            .await
        {
            Ok(at) => ProviderResult::accepted(TransportClass::SmtpAccepted)
                .at_stage(CheckStage::MailFrom)
                .observed(at),
            Err(failure) => classify_smtp_failure(failure, SendPurpose::Canary)
                .observed(OffsetDateTime::now_utc()),
        }
    }
}
