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
    confirm_email_challenge, consume_registration_proof, insert_canary_challenge,
    invalidate_challenge, record_provider_result, reserve_email_challenge, BotTokenCheck,
    ConfirmRequest, ConfirmResult, DependOnEmailTransport,
    DependOnRegistrationEmailChallengeRepository, DependOnRegistrationFlowDirectory,
    DependOnRegistrationGateRepository, DependOnRegistrationProtectionPolicy,
    DependOnRegistrationProtectionRepository, DependOnTurnstileVerifier, EmailTransport,
    NewEmailChallenge, ProofVerification, RegistrationFlowDirectory, RegistrationFlowStatus,
    RegistrationGateRepository, RegistrationProtectionRepository, ReserveOutcome, SendPurpose,
    TurnstileVerifier,
};
use kernel::prelude::entity::{
    evaluate, evaluate_readiness, observation_from_readiness, AuthAccountId, ChallengePurpose,
    CheckScope, ConfigGeneration, EffectiveReadiness, NormalizedEmail, ProofBinding, Protection,
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
const CANARY_FLOW_ID: &str = "canary";

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
    scope: CheckScope,
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
                    scope,
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
            if record_result(self, protection, result, CheckScope::Connection, at).await? {
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
            if record_result(self, Protection::Bot, result, CheckScope::Connection, at).await? {
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
                    if record_result(self, Protection::Email, result, CheckScope::Connection, at)
                        .await?
                    {
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
                    let success = ProviderResult::business_success(TransportClass::SmtpAccepted);
                    if record_result(self, Protection::Email, success, CheckScope::Connection, at)
                        .await?
                    {
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

/// 管理者の検証入力。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AdminVerifyInput {
    /// BOT: widget が発行した有効 token (business 検証)。省略すると負の probe だけ。
    pub token: Option<String>,
    /// メール: 確認する canary challenge の ID と code (roundtrip の確認)。
    pub challenge_id: Option<String>,
    pub code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyDisposition {
    /// 結果を記録した (記録しない `attempt_rejected` を含む)。
    Completed,
    /// canary mailbox へ code を送った。確認 (challenge_id + code) で business success になる。
    PendingConfirmation {
        challenge_id: String,
        expires_at: OffsetDateTime,
    },
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

    /// 実検証の即時実行 (bootstrap・再検証)。HTTP 200 を成功と同一視せず、business 結果と
    /// transport 分類の両方を記録する。
    fn verify_registration_protection<'a>(
        &'a self,
        admin: &'a AuthAccountId,
        protection: Protection,
        input: AdminVerifyInput,
    ) -> impl Future<Output = error_stack::Result<AdminVerifyReport, KernelError>> + Send + 'a {
        async move {
            check_permission(self, admin, &instance_administrate()).await?;
            let at = now();
            let (result, disposition, scope) = match protection {
                Protection::Bot => {
                    if input.challenge_id.is_some() || input.code.is_some() {
                        return Err(Report::new(KernelError::Validation)
                            .attach_printable("challenge_id/code apply to the email protection"));
                    }
                    let result = match input.token.as_deref().map(str::trim) {
                        Some(token) if !token.is_empty() => {
                            self.turnstile_verifier()
                                .verify(&BotTokenCheck {
                                    token,
                                    remote_ip: None,
                                })
                                .await
                        }
                        Some(_) => {
                            return Err(Report::new(KernelError::Validation)
                                .attach_printable("token must not be empty"))
                        }
                        None => self.turnstile_verifier().probe_secret().await,
                    };
                    (result, VerifyDisposition::Completed, CheckScope::Connection)
                }
                Protection::Email => {
                    if input.token.is_some() {
                        return Err(Report::new(KernelError::Validation)
                            .attach_printable("token applies to the bot protection"));
                    }
                    match (input.challenge_id.as_deref(), input.code.as_deref()) {
                        (Some(id), Some(code)) => (
                            self.confirm_canary(id, code, at).await?,
                            VerifyDisposition::Completed,
                            CheckScope::Delivery,
                        ),
                        (None, None) => self.start_canary(at).await?,
                        _ => {
                            return Err(Report::new(KernelError::Validation)
                                .attach_printable("challenge_id and code must be given together"))
                        }
                    }
                }
            };
            let recorded = record_result(self, protection, result, scope, at).await?;
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

    /// メール: 接続・認証 probe → 承認済み canary mailbox へ code を送る。
    fn start_canary<'a>(
        &'a self,
        at: OffsetDateTime,
    ) -> impl Future<
        Output = error_stack::Result<(ProviderResult, VerifyDisposition, CheckScope), KernelError>,
    > + Send
           + 'a {
        async move {
            let transport = self.email_transport();
            let probe = transport.probe_connection().await;
            if probe.outcome != ProviderOutcome::Accepted {
                return Ok((probe, VerifyDisposition::Completed, CheckScope::Connection));
            }
            let Some(canary) = transport.canary_address() else {
                return Ok((
                    probe,
                    VerifyDisposition::CanaryNotConfigured,
                    CheckScope::Connection,
                ));
            };
            let Ok(email) = NormalizedEmail::parse(&canary) else {
                return Ok((
                    ProviderResult::configuration_invalid(
                        ReasonCode::InvalidConfiguration,
                        TransportClass::None,
                    ),
                    VerifyDisposition::Completed,
                    CheckScope::Connection,
                ));
            };
            let snapshot = self.protection_snapshot(Protection::Email);
            let policy = self.email_challenge_policy();
            let id = random_hex(16);
            let code = random_code();
            let new = NewEmailChallenge {
                id: id.clone(),
                purpose: ChallengePurpose::Canary,
                flow_id: CANARY_FLOW_ID.to_string(),
                email_hash: hash_email(&email),
                source_hash: hash_source(CANARY_FLOW_ID),
                generation: snapshot.generation,
                code_hash: hash_code(&id, &code),
                flow_expires_at: at + policy.challenge_ttl,
            };
            let transaction_deps = self.clone();
            let expires_at = self
                .transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        insert_canary_challenge(
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
            let sent = transport
                .send_code(email.as_str(), &code, SendPurpose::Canary)
                .await;
            if sent.outcome == ProviderOutcome::Accepted {
                return Ok((
                    sent,
                    VerifyDisposition::PendingConfirmation {
                        challenge_id: id,
                        expires_at,
                    },
                    CheckScope::Delivery,
                ));
            }
            self.invalidate_registration_challenge(&id).await?;
            Ok((sent, VerifyDisposition::Completed, CheckScope::Delivery))
        }
    }

    /// メール: canary の code 確認。成功だけが business success (generation に束縛)。
    /// canary の確認は proof を発行せず、他の登録者の所有証明にもならない。
    fn confirm_canary<'a>(
        &'a self,
        challenge_id: &'a str,
        code: &'a str,
        at: OffsetDateTime,
    ) -> impl Future<Output = error_stack::Result<ProviderResult, KernelError>> + Send + 'a {
        async move {
            let challenge_id = challenge_id.trim().to_string();
            let code = code.trim().to_string();
            if challenge_id.is_empty()
                || challenge_id.len() > 64
                || code.is_empty()
                || code.len() > MAX_CODE_LEN
            {
                return Ok(ProviderResult::attempt_rejected(
                    ReasonCode::TokenRejected,
                    TransportClass::None,
                ));
            }
            let snapshot = self.protection_snapshot(Protection::Email);
            let policy = self.email_challenge_policy();
            let given_code_hash = hash_code(&challenge_id, &code);
            let transaction_deps = self.clone();
            let generation = snapshot.generation;
            let confirmed = self
                .transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        confirm_email_challenge(
                            transaction_deps.registration_email_challenge_repository(),
                            executor,
                            &ConfirmRequest {
                                id: &challenge_id,
                                expected_purpose: ChallengePurpose::Canary,
                                given_code_hash: &given_code_hash,
                                new_proof_hash: "",
                                current_generation: &generation,
                            },
                            &policy,
                            at,
                        )
                        .await
                    })
                })
                .await?;
            Ok(match confirmed {
                ConfirmResult::Confirmed { .. } => {
                    ProviderResult::business_success(TransportClass::SmtpAccepted)
                }
                ConfirmResult::Rejected(_) => ProviderResult::attempt_rejected(
                    ReasonCode::TokenRejected,
                    TransportClass::None,
                ),
            })
        }
    }
}

impl<T> AdminRegistrationProtectionUseCase for T where
    T: RegistrationProtectionDeps + DependOnPermissionChecker
{
}

#[cfg(test)]
mod tests;
