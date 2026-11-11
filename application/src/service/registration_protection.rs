//! 登録保護の readiness 観測と登録前 email proof の use case (D15 / E15c)。
//!
//! - 実 verification の business 結果を generation に束縛して記録し、鮮度期限付きで評価して
//!   E15a の観測入力 (`RecordRegistrationObservationUseCase`) へ反映する
//! - 登録前 email challenge の発行・確認 (公開) と、proof の照合・消費 (内部。公開 HTTP の書込口なし)
//! - 管理者の readiness 取得と実検証の即時実行
//!
//! 継続 probe の周期実行と hook 時評価は E15d の所有。ここは「1 回の probe を実行して記録する」
//! use case と、時刻を引数に取る評価 (`*_at`) を提供する。

use crate::permission::{check_permission, instance_administrate};
use crate::service::registration_gate::{
    RecordRegistrationObservationUseCase, RegistrationGateView,
};
use error_stack::Report;
use kernel::interfaces::database::{
    DatabaseConnection, DependOnDatabaseConnection, DependOnTransactionManager, TransactionManager,
};
use kernel::interfaces::permission::DependOnPermissionChecker;
use kernel::interfaces::repository::{
    confirm_email_challenge, consume_registration_proof, invalidate_challenge,
    record_provider_result, reserve_email_challenge, BotTokenCheck, ConfirmRequest, ConfirmResult,
    DependOnEmailTransport, DependOnRegistrationEmailChallengeRepository,
    DependOnRegistrationFlowDirectory, DependOnRegistrationGateRepository,
    DependOnRegistrationProtectionPolicy, DependOnRegistrationProtectionRepository,
    DependOnTurnstileVerifier, EmailTransport, NewEmailChallenge, ProofVerification,
    RegistrationFlowDirectory, RegistrationFlowStatus, RegistrationGateRepository,
    RegistrationProtectionRepository, ReserveOutcome, SendPurpose, TurnstileVerifier,
};
use kernel::prelude::entity::{
    evaluate, evaluate_readiness, observation_from_readiness, AuthAccountId, ChallengePurpose,
    ConfigGeneration, EffectiveReadiness, NormalizedEmail, ProofBinding, Protection,
    ProtectionConfigSnapshot, ProtectionProvider, ProviderOutcome, ProviderResult, ReadinessRecord,
    ReadinessStatus, ReasonCode, RegistrationDecision, RegistrationObservation, TransportClass,
};
use kernel::KernelError;
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::future::Future;
use time::OffsetDateTime;

/// PostgreSQL の timestamptz に合わせてマイクロ秒へ丸める (保存前後で値が一致する)。
fn now() -> OffsetDateTime {
    let now = OffsetDateTime::now_utc();
    now.replace_nanosecond(now.nanosecond() / 1000 * 1000)
        .unwrap_or(now)
}

const MAX_FLOW_ID_LEN: usize = 64;
const MAX_CODE_LEN: usize = 32;
const MAX_PROOF_LEN: usize = 256;
const CODE_DIGITS: usize = 8;

fn sha256_hex(domain: &str, parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    for part in parts {
        hasher.update([0u8]);
        hasher.update(part.as_bytes());
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// email は正規化済みの値の hash だけを保存・照合に使う (平文を DB に置かない)。
pub fn hash_email(email: &NormalizedEmail) -> String {
    sha256_hex("emumet:registration-email:v1", &[email.as_str()])
}

fn hash_code(challenge_id: &str, code: &str) -> String {
    sha256_hex("emumet:registration-code:v1", &[challenge_id, code])
}

fn hash_proof(proof: &str) -> String {
    sha256_hex("emumet:registration-proof:v1", &[proof])
}

fn hash_source(source: &str) -> String {
    sha256_hex("emumet:registration-source:v1", &[source])
}

fn random_hex(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut buffer);
    buffer.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// 8 桁の数字 code。剰余バイアスを避けるため u64 の rejection sampling で一様にする。
fn random_code() -> String {
    let limit = 10u64.pow(CODE_DIGITS as u32);
    let zone = u64::MAX - (u64::MAX % limit);
    let mut rng = rand::rngs::OsRng;
    loop {
        let value = rng.next_u64();
        if value < zone {
            return format!("{:0width$}", value % limit, width = CODE_DIGITS);
        }
    }
}

fn valid_flow_id(flow_id: &str) -> bool {
    !flow_id.is_empty()
        && flow_id.len() <= MAX_FLOW_ID_LEN
        && flow_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// 管理者に見せる保護ごとの readiness (secret・token・proof・メールアドレスを含まない)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtectionReadinessView {
    pub protection: Protection,
    pub provider: ProtectionProvider,
    pub enabled: bool,
    pub generation: ConfigGeneration,
    pub fingerprint: String,
    /// 現在時刻・現在設定での評価 (これが E15a の観測入力の根拠)。
    pub effective: EffectiveReadiness,
    pub last_check: Option<LastCheckView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LastCheckView {
    pub checked_at: OffsetDateTime,
    pub outcome: ProviderOutcome,
    pub reason: ReasonCode,
    pub transport: TransportClass,
    pub last_success_at: Option<OffsetDateTime>,
    pub success_valid_until: Option<OffsetDateTime>,
    /// 記録された検証が現在の設定世代のものか (false なら旧設定の記録)。
    pub generation_matches: bool,
}

fn build_view(
    snapshot: &ProtectionConfigSnapshot,
    record: Option<&ReadinessRecord>,
    effective: EffectiveReadiness,
) -> ProtectionReadinessView {
    ProtectionReadinessView {
        protection: snapshot.protection,
        provider: snapshot.provider,
        enabled: snapshot.enabled,
        generation: snapshot.generation.clone(),
        fingerprint: snapshot.fingerprint.clone(),
        effective,
        last_check: record.map(|record| LastCheckView {
            checked_at: record.checked_at,
            outcome: record.check_outcome,
            reason: record.check_reason,
            transport: record.check_transport,
            last_success_at: record.last_success_at,
            success_valid_until: record.success_valid_until,
            generation_matches: record.generation == snapshot.generation,
        }),
    }
}

/// この unit が使う依存の束 (blanket impl)。
pub trait RegistrationProtectionDeps:
    'static
    + Sync
    + Send
    + Clone
    + DependOnDatabaseConnection
    + DependOnTransactionManager
    + DependOnRegistrationProtectionRepository
    + DependOnRegistrationEmailChallengeRepository
    + DependOnRegistrationGateRepository
    + DependOnTurnstileVerifier
    + DependOnEmailTransport
    + DependOnRegistrationFlowDirectory
    + DependOnRegistrationProtectionPolicy
{
    fn protection_snapshot(&self, protection: Protection) -> ProtectionConfigSnapshot {
        match protection {
            Protection::Email => self.email_transport().snapshot(),
            Protection::Bot => self.turnstile_verifier().snapshot(),
        }
    }
}

impl<T> RegistrationProtectionDeps for T where
    T: 'static
        + Sync
        + Send
        + Clone
        + DependOnDatabaseConnection
        + DependOnTransactionManager
        + DependOnRegistrationProtectionRepository
        + DependOnRegistrationEmailChallengeRepository
        + DependOnRegistrationGateRepository
        + DependOnTurnstileVerifier
        + DependOnEmailTransport
        + DependOnRegistrationFlowDirectory
        + DependOnRegistrationProtectionPolicy
{
}

async fn load_views<T: RegistrationProtectionDeps>(
    deps: &T,
    at: OffsetDateTime,
) -> error_stack::Result<[ProtectionReadinessView; 2], KernelError> {
    let policy = deps.readiness_policy();
    let mut connection = deps.database_connection().connection().await?;
    let repository = deps.registration_protection_repository();
    let mut views = Vec::with_capacity(2);
    for protection in Protection::ALL {
        let snapshot = deps.protection_snapshot(protection);
        let record = repository.find(&mut connection, protection).await?;
        let effective = evaluate_readiness(&snapshot, record.as_ref(), at, &policy);
        views.push(build_view(&snapshot, record.as_ref(), effective));
    }
    let bot = views
        .pop()
        .ok_or_else(|| Report::new(KernelError::Internal))?;
    let email = views
        .pop()
        .ok_or_else(|| Report::new(KernelError::Internal))?;
    Ok([email, bot])
}

async fn record_result<T: RegistrationProtectionDeps>(
    deps: &T,
    protection: Protection,
    result: ProviderResult,
    at: OffsetDateTime,
) -> error_stack::Result<bool, KernelError> {
    let snapshot = deps.protection_snapshot(protection);
    let policy = deps.readiness_policy();
    let transaction_deps = deps.clone();
    let recorded = deps
        .transaction_manager()
        .transaction(move |executor| {
            Box::pin(async move {
                record_provider_result(
                    transaction_deps.registration_protection_repository(),
                    executor,
                    &snapshot,
                    &result,
                    at,
                    &policy,
                )
                .await
            })
        })
        .await?;
    Ok(recorded.is_some())
}

/// 両保護を現在の readiness から評価し、E15a の観測入力へ反映する。
/// 一方の `configuration_invalid` で他方の観測を巻き込まない (保護ごとに独立に導く)。
async fn sync_observation<T>(
    deps: &T,
    at: OffsetDateTime,
) -> error_stack::Result<RegistrationGateView, KernelError>
where
    T: RegistrationProtectionDeps,
{
    let [email, bot] = load_views(deps, at).await?;
    let observation = observation_from_readiness(&email.effective, &bot.effective);
    deps.record_registration_observation(observation).await
}

/// Periodic successful API evidence renews a generation that has completed initial bootstrap.
/// SMTP requires DATA (MAIL FROM is insufficient); Turnstile requires an interpreted business
/// response. This never converts a failure or a public recipient attempt into provider health.
async fn renew_bootstrapped_readiness<T: RegistrationProtectionDeps>(
    deps: &T,
    protection: Protection,
    mut result: ProviderResult,
) -> error_stack::Result<ProviderResult, KernelError> {
    if result.outcome != ProviderOutcome::Accepted
        || result.stage != Some(kernel::prelude::entity::CheckStage::Data)
    {
        return Ok(result);
    }
    let snapshot = deps.protection_snapshot(protection);
    let mut connection = deps.database_connection().connection().await?;
    if deps
        .registration_protection_repository()
        .find(&mut connection, protection)
        .await?
        .is_some_and(|record| {
            record.generation == snapshot.generation && record.last_success_at.is_some()
        })
    {
        result.outcome = ProviderOutcome::BusinessSuccess;
        result.reason = ReasonCode::BusinessSuccess;
    }
    Ok(result)
}

/// 検証 API が実際に応答したこと (`Accepted`: BOT は secret が有効という業務応答、メールは DATA 受理) を
/// 当該 generation の business success へ昇格する。証拠 (段階・開始時刻) はそのまま残す。
/// 障害・拒否・接続だけの受理 (`MAIL FROM` まで) は変えない。管理者の検証だけが使う。
fn promote_to_business_success(mut result: ProviderResult) -> ProviderResult {
    if result.outcome == ProviderOutcome::Accepted
        && result.stage == Some(kernel::prelude::entity::CheckStage::Data)
    {
        result.outcome = ProviderOutcome::BusinessSuccess;
        result.reason = ReasonCode::BusinessSuccess;
    }
    result
}

pub trait RegistrationProtectionReadinessUseCase: RegistrationProtectionDeps {
    /// 現在の readiness (評価のみ。何も書き込まない)。
    fn registration_protection_readiness_at(
        &self,
        at: OffsetDateTime,
    ) -> impl Future<Output = error_stack::Result<[ProtectionReadinessView; 2], KernelError>> + Send
    {
        async move { load_views(self, at).await }
    }

    /// 時刻・generation を引数に readiness から導いた観測入力 (副作用なし)。
    fn registration_observation_at(
        &self,
        at: OffsetDateTime,
    ) -> impl Future<Output = error_stack::Result<RegistrationObservation, KernelError>> + Send
    {
        async move {
            let [email, bot] = load_views(self, at).await?;
            Ok(observation_from_readiness(&email.effective, &bot.effective))
        }
    }

    /// 観測入力を E15a へ反映する (`RecordRegistrationObservationUseCase` 経由のみ)。
    fn sync_registration_observation_at(
        &self,
        at: OffsetDateTime,
    ) -> impl Future<Output = error_stack::Result<RegistrationGateView, KernelError>> + Send {
        async move { sync_observation(self, at).await }
    }

    fn sync_registration_observation(
        &self,
    ) -> impl Future<Output = error_stack::Result<RegistrationGateView, KernelError>> + Send {
        async move { sync_observation(self, now()).await }
    }

    /// 1 回の probe を実行して結果を記録し、観測入力を反映する。
    /// BOT: secret の負の probe (business success にはならない)。
    /// メール: 接続・認証 probe (メールは送らない。SMTP 受理は business success にならない)。
    fn run_registration_protection_probe_at(
        &self,
        protection: Protection,
        at: OffsetDateTime,
    ) -> impl Future<Output = error_stack::Result<ProviderResult, KernelError>> + Send {
        async move {
            let result = match protection {
                Protection::Bot => self.turnstile_verifier().probe_secret().await,
                Protection::Email => self.email_transport().probe_connection().await,
            };
            if record_result(self, protection, result, at).await? {
                sync_observation(self, at).await?;
            }
            Ok(result)
        }
    }

    fn run_registration_protection_probe(
        &self,
        protection: Protection,
    ) -> impl Future<Output = error_stack::Result<ProviderResult, KernelError>> + Send {
        async move {
            self.run_registration_protection_probe_at(protection, now())
                .await
        }
    }

    /// Fixed approved mailbox only. DATA acceptance clears covered delivery faults and renews
    /// business freshness after initial generation bootstrap; it cannot bootstrap a new generation.
    fn revalidate_registration_email_delivery(
        &self,
    ) -> impl Future<Output = error_stack::Result<ProviderResult, KernelError>> + Send {
        async move {
            let transport = self.email_transport();
            let result = match transport.canary_address() {
                Some(address) if NormalizedEmail::parse(&address).is_ok() => {
                    transport
                        .send_code(&address, &random_code(), SendPurpose::Canary)
                        .await
                }
                _ => return Ok(ProviderResult::accepted(TransportClass::None).without_evidence()),
            };
            let result = renew_bootstrapped_readiness(self, Protection::Email, result).await?;
            let at = now();
            if record_result(self, Protection::Email, result, at).await? {
                sync_observation(self, at).await?;
            }
            Ok(result)
        }
    }

    /// Global persistent due time plus transaction-scoped exclusion. The outer transaction owns
    /// only the schedule lock; short readiness/gate transactions are separate. Cancellation or
    /// process death releases the lock and rolls back the reservation. No provider verdict is cached.
    fn run_registration_protection_maintenance(
        &self,
        interval: time::Duration,
    ) -> impl Future<Output = error_stack::Result<bool, KernelError>> + Send {
        async move {
            if interval <= time::Duration::ZERO
                || interval >= self.readiness_policy().probe_max_age / 2
            {
                return Err(Report::new(KernelError::Validation).attach_printable(
                    "Maintenance interval must be positive and less than half probe_max_age",
                ));
            }
            let deps = self.clone();
            self.transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        if !deps
                            .registration_protection_repository()
                            .begin_maintenance_cycle(executor, now(), interval)
                            .await?
                        {
                            return Ok(false);
                        }
                        deps.run_registration_protection_probe(Protection::Email)
                            .await?;
                        let bot_result = deps.turnstile_verifier().probe_secret().await;
                        let bot_result =
                            renew_bootstrapped_readiness(&deps, Protection::Bot, bot_result)
                                .await?;
                        record_result(&deps, Protection::Bot, bot_result, now()).await?;
                        deps.revalidate_registration_email_delivery().await?;
                        // Always sync, even when no provider result was recorded: expired business
                        // evidence and generation changes must be visible without an incoming request.
                        deps.sync_registration_observation().await?;
                        Ok(true)
                    })
                })
                .await
        }
    }

    /// 登録者の BOT token を検証する (hook から呼ぶ内部 use case)。business success は当該 generation
    /// の readiness に記録され、provider 起因の失敗 (設定不正・一時障害) も記録される。
    /// 利用者の不正 token (`attempt_rejected`) は readiness を変えない。
    fn verify_registration_bot_token(
        &self,
        token: &str,
        remote_ip: Option<&str>,
    ) -> impl Future<Output = error_stack::Result<BotTokenVerdict, KernelError>> + Send {
        let token = token.to_string();
        let remote_ip = remote_ip.map(str::to_string);
        async move {
            let result = self
                .turnstile_verifier()
                .verify(&BotTokenCheck {
                    token: &token,
                    remote_ip: remote_ip.as_deref(),
                })
                .await;
            let at = now();
            if record_result(self, Protection::Bot, result, at).await? {
                sync_observation(self, at).await?;
            }
            Ok(BotTokenVerdict::from_result(&result))
        }
    }
}

impl<T> RegistrationProtectionReadinessUseCase for T where T: RegistrationProtectionDeps {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BotTokenVerdict {
    Verified,
    /// 当該 token だけの拒否 (provider 全体の障害ではない)。
    Rejected(ReasonCode),
    /// provider 側の問題 (設定不正・一時障害)。登録を許可する根拠にならない。
    ProviderFailure(ProviderOutcome, ReasonCode),
}

impl BotTokenVerdict {
    fn from_result(result: &ProviderResult) -> Self {
        match result.outcome {
            ProviderOutcome::BusinessSuccess => Self::Verified,
            ProviderOutcome::AttemptRejected => Self::Rejected(result.reason),
            // 負の probe の `Accepted` は token の検証成功ではない。
            outcome => Self::ProviderFailure(outcome, result.reason),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssueOutcome {
    /// 発行した。送信を試みた発行は、受理・宛先拒否・一時障害・provider 障害のいずれでも同じ応答で、
    /// 宛先の状態を区別させない (届かなかった challenge は内部で失効させる)。
    Issued {
        challenge_id: String,
        expires_at: OffsetDateTime,
    },
    InvalidEmail,
    InvalidFlow,
    RateLimited {
        retry_after_secs: i64,
    },
    /// 送信前に判定できる拒否 (宛先に依存しない): gate が受付中でない・メール保護が operational でない・
    /// Kratos に到達できない。任意宛先へ送らない。理由は外部に区別して返さない。
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmOutcome {
    Confirmed {
        proof: String,
        expires_at: OffsetDateTime,
    },
    /// unknown id・code 不一致・期限切れ・試行超過・世代変更を区別しない。
    Rejected,
}

pub struct IssueRequest<'a> {
    pub email: &'a str,
    pub flow_id: &'a str,
    /// browser flow の実期限を Kratos に照会するための CSRF cookie (`Cookie` ヘッダー値)。
    pub flow_cookie: Option<&'a str>,
    /// 送信元の識別子 (接続元アドレス。信頼できないヘッダーは使わない)。
    pub source: &'a str,
}

async fn registration_email_ready<T: RegistrationProtectionDeps>(
    deps: &T,
    at: OffsetDateTime,
) -> error_stack::Result<bool, KernelError> {
    let state = {
        let mut connection = deps.database_connection().connection().await?;
        deps.registration_gate_repository()
            .load(&mut connection)
            .await?
    };
    if !matches!(evaluate(&state), RegistrationDecision::Accept { .. }) {
        return Ok(false);
    }
    let [email, _] = load_views(deps, at).await?;
    Ok(email.effective.status == ReadinessStatus::Operational)
}

pub trait RegistrationEmailChallengeUseCase: RegistrationProtectionDeps {
    /// 登録前 email challenge を発行する (公開 API から呼ぶ)。
    ///
    /// メール爆弾・踏み台対策として、gate が受付中かつメール保護が operational のときだけ送る。
    /// 実 registration flow の実在・期限を確認し、flow / email / generation に束縛する。
    /// 応答は宛先の登録状況・宛先拒否で変わらない (user enumeration の防止)。
    fn issue_registration_email_challenge_at(
        &self,
        request: &IssueRequest<'_>,
        at: OffsetDateTime,
    ) -> impl Future<Output = error_stack::Result<IssueOutcome, KernelError>> + Send {
        let raw_email = request.email.to_string();
        let flow_id = request.flow_id.to_string();
        let flow_cookie = request.flow_cookie.map(str::to_string);
        let source = request.source.to_string();
        async move {
            let Ok(email) = NormalizedEmail::parse(&raw_email) else {
                return Ok(IssueOutcome::InvalidEmail);
            };
            if !valid_flow_id(&flow_id) {
                return Ok(IssueOutcome::InvalidFlow);
            }
            if !registration_email_ready(self, at).await? {
                return Ok(IssueOutcome::Unavailable);
            }
            let flow_expires_at = match self
                .registration_flow_directory()
                .lookup(&flow_id, flow_cookie.as_deref())
                .await
            {
                Ok(RegistrationFlowStatus::Active { expires_at }) if expires_at > at => expires_at,
                Ok(_) => return Ok(IssueOutcome::InvalidFlow),
                Err(_) => return Ok(IssueOutcome::Unavailable),
            };
            let snapshot = self.protection_snapshot(Protection::Email);
            let policy = self.email_challenge_policy();
            let id = random_hex(16);
            let code = random_code();
            let new = NewEmailChallenge {
                id: id.clone(),
                purpose: ChallengePurpose::Registration,
                flow_id: flow_id.clone(),
                email_hash: hash_email(&email),
                source_hash: hash_source(&source),
                generation: snapshot.generation.clone(),
                code_hash: hash_code(&id, &code),
                flow_expires_at,
            };
            let transaction_deps = self.clone();
            let reserved = self
                .transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        reserve_email_challenge(
                            transaction_deps.registration_email_challenge_repository(),
                            executor,
                            new,
                            &policy,
                            at,
                        )
                        .await
                    })
                })
                .await?;
            let expires_at = match reserved {
                ReserveOutcome::Limited { retry_after, .. } => {
                    return Ok(IssueOutcome::RateLimited {
                        retry_after_secs: retry_after.whole_seconds().max(1),
                    })
                }
                ReserveOutcome::Reserved { expires_at } => expires_at,
            };

            let result = self
                .email_transport()
                .send_code(email.as_str(), &code, SendPurpose::Registration)
                .await;
            // 送信を試みた発行の外部応答は結果によらず同一にする。宛先拒否 (RCPT 550) と宛先依存の
            // 一時拒否 (RCPT 450 など) を公開応答で区別させない (宛先の状態の列挙を防ぐ)。
            // readiness に記録するのは、宛先を提示する前の段階 (接続・STARTTLS・認証・MAIL FROM) の失敗だけ。
            // 宛先提示後 (RCPT 以降) の失敗は adapter が必ず attempt_rejected にするので、公開要求の失敗を
            // readiness に積み上げて後続の応答 (202/503) から先行宛先の状態が分かることはない。
            // provider の健全性は宛先を伴わない独立 probe と管理者の canary で判定する。
            if !matches!(
                result.outcome,
                ProviderOutcome::Accepted | ProviderOutcome::BusinessSuccess
            ) {
                self.invalidate_registration_challenge(&id).await?;
                if matches!(
                    result.outcome,
                    ProviderOutcome::ConfigurationInvalid | ProviderOutcome::TemporarilyUnavailable
                ) {
                    tracing::warn!(
                        outcome = result.outcome.as_str(),
                        reason = result.reason.as_str(),
                        "registration email challenge could not be sent"
                    );
                    if record_result(self, Protection::Email, result, at).await? {
                        sync_observation(self, at).await?;
                    }
                }
            }
            Ok(IssueOutcome::Issued {
                challenge_id: id,
                expires_at,
            })
        }
    }

    fn issue_registration_email_challenge(
        &self,
        request: &IssueRequest<'_>,
    ) -> impl Future<Output = error_stack::Result<IssueOutcome, KernelError>> + Send {
        async move {
            self.issue_registration_email_challenge_at(request, now())
                .await
        }
    }

    fn invalidate_registration_challenge(
        &self,
        id: &str,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        let id = id.to_string();
        async move {
            let transaction_deps = self.clone();
            self.transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        invalidate_challenge(
                            transaction_deps.registration_email_challenge_repository(),
                            executor,
                            &id,
                        )
                        .await
                    })
                })
                .await
        }
    }

    /// code を確認し、一致したときだけ proof を発行する (公開 API から呼ぶ)。
    /// 確認成功は実際の配送・確認の business success なので、メール保護の readiness にも記録する。
    fn confirm_registration_email_challenge_at(
        &self,
        challenge_id: &str,
        code: &str,
        at: OffsetDateTime,
    ) -> impl Future<Output = error_stack::Result<ConfirmOutcome, KernelError>> + Send {
        let challenge_id = challenge_id.trim().to_string();
        let code = code.trim().to_string();
        async move {
            if challenge_id.is_empty()
                || challenge_id.len() > 64
                || code.is_empty()
                || code.len() > MAX_CODE_LEN
            {
                return Ok(ConfirmOutcome::Rejected);
            }
            let snapshot = self.protection_snapshot(Protection::Email);
            let policy = self.email_challenge_policy();
            let proof = random_hex(32);
            let given_code_hash = hash_code(&challenge_id, &code);
            let proof_hash = hash_proof(&proof);
            let transaction_deps = self.clone();
            let generation = snapshot.generation.clone();
            let confirmed = self
                .transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        confirm_email_challenge(
                            transaction_deps.registration_email_challenge_repository(),
                            executor,
                            &ConfirmRequest {
                                id: &challenge_id,
                                expected_purpose: ChallengePurpose::Registration,
                                given_code_hash: &given_code_hash,
                                new_proof_hash: &proof_hash,
                                current_generation: &generation,
                            },
                            &policy,
                            at,
                        )
                        .await
                    })
                })
                .await?;
            match confirmed {
                ConfirmResult::Confirmed {
                    proof_expires_at: Some(expires_at),
                    ..
                } => {
                    let success = ProviderResult::business_success(TransportClass::SmtpAccepted)
                        .without_evidence();
                    if record_result(self, Protection::Email, success, at).await? {
                        sync_observation(self, at).await?;
                    }
                    Ok(ConfirmOutcome::Confirmed { proof, expires_at })
                }
                _ => Ok(ConfirmOutcome::Rejected),
            }
        }
    }

    fn confirm_registration_email_challenge(
        &self,
        challenge_id: &str,
        code: &str,
    ) -> impl Future<Output = error_stack::Result<ConfirmOutcome, KernelError>> + Send {
        async move {
            self.confirm_registration_email_challenge_at(challenge_id, code, now())
                .await
        }
    }

    /// proof を実 context (flow / email) と現在の generation に照合して原子的に消費する。
    /// **内部 use case**: 公開 HTTP の書込口にしない (hook への配線は E15d)。
    /// 同じ束縛の再利用窓内の retry にだけ同じ検証済み事実を返し、gate の許可 verdict は含まない。
    fn verify_registration_email_proof_at(
        &self,
        flow_id: &str,
        email: &str,
        proof: &str,
        at: OffsetDateTime,
    ) -> impl Future<Output = error_stack::Result<ProofVerification, KernelError>> + Send {
        let flow_id = flow_id.to_string();
        let email = email.to_string();
        let proof = proof.trim().to_string();
        async move {
            use kernel::prelude::entity::ProofRejection;
            let Ok(email) = NormalizedEmail::parse(&email) else {
                return Ok(ProofVerification::Rejected(ProofRejection::EmailMismatch));
            };
            if proof.is_empty() || proof.len() > MAX_PROOF_LEN {
                return Ok(ProofVerification::Rejected(ProofRejection::Unknown));
            }
            let snapshot = self.protection_snapshot(Protection::Email);
            let binding = ProofBinding {
                flow_id,
                email_hash: hash_email(&email),
                generation: snapshot.generation,
            };
            let proof_hash = hash_proof(&proof);
            let policy = self.email_challenge_policy();
            let transaction_deps = self.clone();
            self.transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        consume_registration_proof(
                            transaction_deps.registration_email_challenge_repository(),
                            executor,
                            &proof_hash,
                            &binding,
                            &policy,
                            at,
                        )
                        .await
                    })
                })
                .await
        }
    }

    fn verify_registration_email_proof(
        &self,
        flow_id: &str,
        email: &str,
        proof: &str,
    ) -> impl Future<Output = error_stack::Result<ProofVerification, KernelError>> + Send {
        async move {
            self.verify_registration_email_proof_at(flow_id, email, proof, now())
                .await
        }
    }
}

impl<T> RegistrationEmailChallengeUseCase for T where T: RegistrationProtectionDeps {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyDisposition {
    /// 結果を記録した (記録しない `attempt_rejected` を含む)。
    Completed,
    /// 承認済み canary mailbox が未設定。メールは unverified のまま。
    CanaryNotConfigured,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminVerifyReport {
    /// business 結果と transport 分類。
    pub result: ProviderResult,
    /// readiness へ記録したか (`attempt_rejected` は記録しない)。
    pub recorded: bool,
    pub disposition: VerifyDisposition,
    pub readiness: ProtectionReadinessView,
}

pub trait AdminRegistrationProtectionUseCase:
    RegistrationProtectionDeps + DependOnPermissionChecker
{
    fn list_registration_protections<'a>(
        &'a self,
        admin: &'a AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<[ProtectionReadinessView; 2], KernelError>> + Send + 'a
    {
        async move {
            check_permission(self, admin, &instance_administrate()).await?;
            load_views(self, now()).await
        }
    }

    /// 実検証の即時実行 (初回 bootstrap・再検証)。入力は取らない (d15-q05: 設定が有効なだけでは
    /// 足りず、有効な保護について実際に検証 API が応答することを確認して初めて準備済み)。
    ///
    /// - BOT: 有効でないダミー token で Siteverify を呼び、**secret が有効** (`invalid-input-response`
    ///   など token 起因の拒否) と応答したときだけ business success。`invalid-input-secret` /
    ///   `missing-input-secret` は設定不正、429/5xx/timeout/不正応答は一時障害で、HTTP 200 だけでは成功にしない。
    /// - メール: 固定の canary 宛へ実送信し、SMTP の **DATA 受理** を business success とする。
    ///   RCPT/DATA を含む各段階の失敗は障害として分類・記録する。
    ///
    /// 成功は generation に束縛され、SMTP の対話の段階・開始時刻を証拠として持つ
    /// (既知の障害を解除できるのは、それを覆う段階の、障害より後の証拠だけ)。
    /// 周期実行 (`renew_bootstrapped_readiness`) は新しい generation を bootstrap できない。
    fn verify_registration_protection<'a>(
        &'a self,
        admin: &'a AuthAccountId,
        protection: Protection,
    ) -> impl Future<Output = error_stack::Result<AdminVerifyReport, KernelError>> + Send + 'a {
        async move {
            check_permission(self, admin, &instance_administrate()).await?;
            let at = now();
            let (result, disposition) = match protection {
                Protection::Bot => (
                    promote_to_business_success(self.turnstile_verifier().probe_secret().await),
                    VerifyDisposition::Completed,
                ),
                Protection::Email => self.verify_canary_delivery().await?,
            };
            let recorded = record_result(self, protection, result, at).await?;
            if recorded {
                sync_observation(self, at).await?;
            }
            let views = load_views(self, at).await?;
            let readiness = match protection {
                Protection::Email => views[0].clone(),
                Protection::Bot => views[1].clone(),
            };
            Ok(AdminVerifyReport {
                result,
                recorded,
                disposition,
                readiness,
            })
        }
    }

    /// メール: 承認済み canary mailbox への実送信。DATA 受理が business success になる。
    /// canary が未設定なら接続 probe の結果だけを記録し、メールは unverified のまま。
    /// 宛先は固定で、登録者の入力を宛先にしない。proof も challenge も発行しない。
    fn verify_canary_delivery<'a>(
        &'a self,
    ) -> impl Future<Output = error_stack::Result<(ProviderResult, VerifyDisposition), KernelError>>
           + Send
           + 'a {
        async move {
            let transport = self.email_transport();
            let Some(canary) = transport.canary_address() else {
                let probe = transport.probe_connection().await;
                // 接続・認証の失敗はその分類で報告する。canary 未設定は接続が受理されたときだけ。
                let disposition = if probe.outcome == ProviderOutcome::Accepted {
                    VerifyDisposition::CanaryNotConfigured
                } else {
                    VerifyDisposition::Completed
                };
                return Ok((probe, disposition));
            };
            let Ok(email) = NormalizedEmail::parse(&canary) else {
                return Ok((
                    ProviderResult::configuration_invalid(
                        ReasonCode::InvalidConfiguration,
                        TransportClass::None,
                    ),
                    VerifyDisposition::Completed,
                ));
            };
            let sent = transport
                .send_code(email.as_str(), &random_code(), SendPurpose::Canary)
                .await;
            Ok((
                promote_to_business_success(sent),
                VerifyDisposition::Completed,
            ))
        }
    }
}

impl<T> AdminRegistrationProtectionUseCase for T where
    T: RegistrationProtectionDeps + DependOnPermissionChecker
{
}

#[cfg(test)]
mod tests;
