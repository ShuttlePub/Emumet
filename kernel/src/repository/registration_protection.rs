use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::{
    apply_provider_result, decide_confirmation, decide_proof_use, evaluate_send_limits,
    ChallengePurpose, ChallengeRejection, ChallengeStatus, ConfigGeneration, ConfirmDecision,
    EmailChallenge, EmailChallengePolicy, ProofBinding, ProofDecision, ProofRejection, Protection,
    ProtectionConfigSnapshot, ProviderResult, ReadinessPolicy, ReadinessRecord, SendCounts,
    SendLimitDecision, SendLimitScope,
};
use crate::KernelError;
use std::future::Future;
use time::OffsetDateTime;

/// readiness 記録の永続化ポート (保護ごとに 1 行)。
pub trait RegistrationProtectionRepository: Sync + Send + 'static {
    type Connection: Connection;

    fn find(
        &self,
        executor: &mut Self::Connection,
        protection: Protection,
    ) -> impl Future<Output = error_stack::Result<Option<ReadinessRecord>, KernelError>> + Send;

    /// 行ロック付きで読む (read-modify-write は transaction 内でこちらを使う)。
    fn find_for_update(
        &self,
        executor: &mut Self::Connection,
        protection: Protection,
    ) -> impl Future<Output = error_stack::Result<Option<ReadinessRecord>, KernelError>> + Send;

    fn upsert(
        &self,
        executor: &mut Self::Connection,
        record: &ReadinessRecord,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnRegistrationProtectionRepository:
    Sync + Send + DependOnDatabaseConnection
{
    type RegistrationProtectionRepository: RegistrationProtectionRepository<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn registration_protection_repository(&self) -> &Self::RegistrationProtectionRepository;
}

/// email challenge / proof / 送信 rate limit の永続化ポート。
pub trait RegistrationEmailChallengeRepository: Sync + Send + 'static {
    type Connection: Connection;

    /// 送信 rate limit の判定〜記録を直列化する transaction スコープの lock。
    /// 呼び出しは `count_sends` → `record_send` と同じ transaction 内で行う。
    fn lock_send_rate_limit(
        &self,
        executor: &mut Self::Connection,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn count_sends(
        &self,
        executor: &mut Self::Connection,
        email_hash: &str,
        flow_id: &str,
        source_hash: &str,
        now: OffsetDateTime,
    ) -> impl Future<Output = error_stack::Result<SendCounts, KernelError>> + Send;

    fn record_send(
        &self,
        executor: &mut Self::Connection,
        email_hash: &str,
        flow_id: &str,
        source_hash: &str,
        sent_at: OffsetDateTime,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn insert(
        &self,
        executor: &mut Self::Connection,
        challenge: &EmailChallenge,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn find_for_update(
        &self,
        executor: &mut Self::Connection,
        id: &str,
    ) -> impl Future<Output = error_stack::Result<Option<EmailChallenge>, KernelError>> + Send;

    fn find_by_proof_hash_for_update(
        &self,
        executor: &mut Self::Connection,
        proof_hash: &str,
    ) -> impl Future<Output = error_stack::Result<Option<EmailChallenge>, KernelError>> + Send;

    /// 可変フィールド (attempts / status / confirmed_at / proof / consumed_at) を保存する。
    fn save(
        &self,
        executor: &mut Self::Connection,
        challenge: &EmailChallenge,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    /// 期限を十分過ぎた challenge と古い送信記録を削除する。削除件数を返す。
    fn purge_expired(
        &self,
        executor: &mut Self::Connection,
        before: OffsetDateTime,
    ) -> impl Future<Output = error_stack::Result<u64, KernelError>> + Send;
}

pub trait DependOnRegistrationEmailChallengeRepository:
    Sync + Send + DependOnDatabaseConnection
{
    type RegistrationEmailChallengeRepository: RegistrationEmailChallengeRepository<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn registration_email_challenge_repository(
        &self,
    ) -> &Self::RegistrationEmailChallengeRepository;
}

/// BOT token の検証要求。token は検証 provider へ送るだけで保存・ログ出力しない。
pub struct BotTokenCheck<'a> {
    pub token: &'a str,
    pub remote_ip: Option<&'a str>,
}

/// Turnstile Siteverify adapter のポート。
pub trait TurnstileVerifier: Sync + Send + 'static {
    /// 現在の設定のスナップショット (generation・fingerprint は secret を含まない)。
    fn snapshot(&self) -> ProtectionConfigSnapshot;

    /// 登録者の token を検証する。`success:true` + hostname/action/期限/一回性 の照合まで行い、
    /// business success のときだけ `BusinessSuccess` を返す。HTTP 200 の `success:false` は失敗。
    fn verify(&self, check: &BotTokenCheck<'_>) -> impl Future<Output = ProviderResult> + Send;

    /// 負の probe: 有効な token を持たずに secret が受理されていることだけを確認する
    /// (`invalid-input-response` 等 → `Accepted`)。business success ではないので operational の
    /// 根拠にはならない。
    fn probe_secret(&self) -> impl Future<Output = ProviderResult> + Send;
}

pub trait DependOnTurnstileVerifier: Sync + Send {
    type TurnstileVerifier: TurnstileVerifier;

    fn turnstile_verifier(&self) -> &Self::TurnstileVerifier;
}

/// メール送信の目的。送信失敗を readiness の根拠にしてよいかが目的で変わる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendPurpose {
    /// 公開要求の登録者宛て。宛先を提示した後 (RCPT 以降) の失敗は宛先に依存し得るので、観測の根拠にしない
    /// (`attempt_rejected`)。公開応答から先行宛先の状態を判別させないため。
    Registration,
    /// 管理者の canary (固定の承認済み宛先)。宛先は固定で攻撃者が選べないので、RCPT/DATA/timeout を含む
    /// 配送失敗を provider の障害 (一時障害/設定不正) として扱う。
    Canary,
}

/// メール送信 (SMTP) adapter のポート。
pub trait EmailTransport: Sync + Send + 'static {
    fn snapshot(&self) -> ProtectionConfigSnapshot;

    /// 承認済み canary mailbox (設定されていれば)。
    fn canary_address(&self) -> Option<String>;

    /// code を含むメールを送る。成功は `Accepted` (SMTP 受理) までで所有確認ではない。
    /// 宛先拒否 (550 等) は `AttemptRejected`、認証・設定不正は `ConfigurationInvalid`、
    /// 接続失敗・timeout・4xx は `TemporarilyUnavailable`。code・本文・secret はログに出さない。
    fn send_code(
        &self,
        to: &str,
        code: &str,
        purpose: SendPurpose,
    ) -> impl Future<Output = ProviderResult> + Send;

    /// メールを送らずに接続・認証だけ確認する probe (成功は `Accepted`)。
    fn probe_connection(&self) -> impl Future<Output = ProviderResult> + Send;
}

pub trait DependOnEmailTransport: Sync + Send {
    type EmailTransport: EmailTransport;

    fn email_transport(&self) -> &Self::EmailTransport;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationFlowStatus {
    /// 有効な registration flow。`expires_at` 以降は使えない。
    Active { expires_at: OffsetDateTime },
    /// 存在しない・期限切れ・registration flow ではない・実際の期限を確認できない。
    NotUsable,
}

/// Kratos の registration flow を実在確認するポート (challenge を実 flow に束縛するため)。
pub trait RegistrationFlowDirectory: Sync + Send + 'static {
    /// Kratos に到達できない場合は `Err` (呼び出し側は一時障害として扱う)。
    /// `browser_cookie` は browser flow の照会に必要な Kratos の CSRF cookie (`Cookie` ヘッダー値)。
    /// cookie がなく中身を確認できない flow は `NotUsable` とし、期限を推測して許可しない。
    fn lookup(
        &self,
        flow_id: &str,
        browser_cookie: Option<&str>,
    ) -> impl Future<Output = error_stack::Result<RegistrationFlowStatus, KernelError>> + Send;
}

pub trait DependOnRegistrationFlowDirectory: Sync + Send {
    type RegistrationFlowDirectory: RegistrationFlowDirectory;

    fn registration_flow_directory(&self) -> &Self::RegistrationFlowDirectory;
}

/// 現在の readiness 評価ポリシーと email challenge ポリシー (サーバー設定から構築)。
pub trait DependOnRegistrationProtectionPolicy: Sync + Send {
    fn readiness_policy(&self) -> crate::entity::ReadinessPolicy;
    fn email_challenge_policy(&self) -> crate::entity::EmailChallengePolicy;
}

/// 検証結果を readiness 記録へ畳み込んで保存する。transaction 内で呼ぶこと。
/// `attempt_rejected` は何も保存しない (`None`)。保存したときは新しい記録を返す。
pub async fn record_provider_result<R: RegistrationProtectionRepository>(
    repository: &R,
    executor: &mut R::Connection,
    config: &ProtectionConfigSnapshot,
    result: &ProviderResult,
    at: OffsetDateTime,
    policy: &ReadinessPolicy,
) -> error_stack::Result<Option<ReadinessRecord>, KernelError> {
    let existing = repository
        .find_for_update(executor, config.protection)
        .await?;
    let record = apply_provider_result(existing.as_ref(), config, result, at, policy);
    if let Some(record) = &record {
        repository.upsert(executor, record).await?;
    }
    Ok(record)
}

/// 定数時間の文字列比較 (hash の hex 同士を比べる)。長さが違えば false。
pub fn constant_time_eq(left: &str, right: &str) -> bool {
    let (left, right) = (left.as_bytes(), right.as_bytes());
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |acc, (l, r)| acc | (l ^ r))
        == 0
}

/// 新しい challenge の入力。code / email / source は呼び出し側が hash 済みで渡す。
#[derive(Debug, Clone)]
pub struct NewEmailChallenge {
    pub id: String,
    pub purpose: ChallengePurpose,
    pub flow_id: String,
    pub email_hash: String,
    pub source_hash: String,
    pub generation: ConfigGeneration,
    pub code_hash: String,
    pub flow_expires_at: OffsetDateTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReserveOutcome {
    Reserved {
        expires_at: OffsetDateTime,
    },
    Limited {
        scope: SendLimitScope,
        retry_after: time::Duration,
    },
}

/// 送信枠の確保 (rate limit の判定 + 記録) と challenge の保存を 1 transaction で行う。
/// `lock_send_rate_limit` で直列化するため、並行する発行が上限を超えて通ることはない。
/// 期限を十分過ぎた行の掃除も同じ transaction で行う。
pub async fn reserve_email_challenge<R: RegistrationEmailChallengeRepository>(
    repository: &R,
    executor: &mut R::Connection,
    new: NewEmailChallenge,
    policy: &EmailChallengePolicy,
    now: OffsetDateTime,
) -> error_stack::Result<ReserveOutcome, KernelError> {
    repository.lock_send_rate_limit(executor).await?;
    let counts = repository
        .count_sends(
            executor,
            &new.email_hash,
            &new.flow_id,
            &new.source_hash,
            now,
        )
        .await?;
    if let SendLimitDecision::Limited { scope, retry_after } =
        evaluate_send_limits(&counts, &policy.limits, now)
    {
        return Ok(ReserveOutcome::Limited { scope, retry_after });
    }
    repository
        .record_send(
            executor,
            &new.email_hash,
            &new.flow_id,
            &new.source_hash,
            now,
        )
        .await?;
    let expires_at = (now + policy.challenge_ttl).min(new.flow_expires_at);
    let challenge = EmailChallenge {
        id: new.id,
        purpose: new.purpose,
        flow_id: new.flow_id,
        email_hash: new.email_hash,
        generation: new.generation,
        code_hash: new.code_hash,
        attempts: 0,
        max_attempts: policy.max_attempts,
        status: ChallengeStatus::Pending,
        created_at: now,
        expires_at,
        flow_expires_at: new.flow_expires_at,
        confirmed_at: None,
        proof_hash: None,
        proof_expires_at: None,
        consumed_at: None,
    };
    repository.insert(executor, &challenge).await?;
    repository
        .purge_expired(executor, now - time::Duration::days(1))
        .await?;
    Ok(ReserveOutcome::Reserved { expires_at })
}

/// 管理者の canary challenge の保存 (rate limit の対象外。proof は発行されない)。
pub async fn insert_canary_challenge<R: RegistrationEmailChallengeRepository>(
    repository: &R,
    executor: &mut R::Connection,
    new: NewEmailChallenge,
    policy: &EmailChallengePolicy,
    now: OffsetDateTime,
) -> error_stack::Result<OffsetDateTime, KernelError> {
    let expires_at = (now + policy.challenge_ttl).min(new.flow_expires_at);
    let challenge = EmailChallenge {
        id: new.id,
        purpose: ChallengePurpose::Canary,
        flow_id: new.flow_id,
        email_hash: new.email_hash,
        generation: new.generation,
        code_hash: new.code_hash,
        attempts: 0,
        max_attempts: policy.max_attempts,
        status: ChallengeStatus::Pending,
        created_at: now,
        expires_at,
        flow_expires_at: new.flow_expires_at,
        confirmed_at: None,
        proof_hash: None,
        proof_expires_at: None,
        consumed_at: None,
    };
    repository.insert(executor, &challenge).await?;
    Ok(expires_at)
}

/// 送信に失敗した challenge を失効させる (届いていない code を確認させない)。
pub async fn invalidate_challenge<R: RegistrationEmailChallengeRepository>(
    repository: &R,
    executor: &mut R::Connection,
    id: &str,
) -> error_stack::Result<(), KernelError> {
    if let Some(mut challenge) = repository.find_for_update(executor, id).await? {
        if challenge.status == ChallengeStatus::Pending {
            challenge.status = ChallengeStatus::Invalidated;
            repository.save(executor, &challenge).await?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmResult {
    /// code が一致した。registration challenge なら `proof_expires_at` が入る。
    Confirmed {
        purpose: ChallengePurpose,
        proof_expires_at: Option<OffsetDateTime>,
    },
    /// 外部には区別せず「確認できない」として扱う (unknown id / 不一致 / 期限切れ / 試行超過 / 世代変更)。
    Rejected(ConfirmRejection),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmRejection {
    UnknownChallenge,
    PurposeMismatch,
    Challenge(ChallengeRejection),
    WrongCode { attempts: i32, invalidated: bool },
}

/// code 確認の入力。code と新しい proof は hash 済みで渡す。
#[derive(Debug, Clone, Copy)]
pub struct ConfirmRequest<'a> {
    pub id: &'a str,
    pub expected_purpose: ChallengePurpose,
    pub given_code_hash: &'a str,
    pub new_proof_hash: &'a str,
    pub current_generation: &'a ConfigGeneration,
}

/// code の確認。row lock の下で判定するため、並行する確認でも proof が発行されるのは 1 回だけ。
/// 不一致は試行を消費して**保存する** (呼び出し側は `Ok` を返して commit すること)。
/// `proof` (hash) は registration challenge が確認されたときだけ保存する。
pub async fn confirm_email_challenge<R: RegistrationEmailChallengeRepository>(
    repository: &R,
    executor: &mut R::Connection,
    request: &ConfirmRequest<'_>,
    policy: &EmailChallengePolicy,
    now: OffsetDateTime,
) -> error_stack::Result<ConfirmResult, KernelError> {
    let ConfirmRequest {
        id,
        expected_purpose,
        given_code_hash,
        new_proof_hash,
        current_generation,
    } = *request;
    let Some(mut challenge) = repository.find_for_update(executor, id).await? else {
        return Ok(ConfirmResult::Rejected(ConfirmRejection::UnknownChallenge));
    };
    if challenge.purpose != expected_purpose {
        return Ok(ConfirmResult::Rejected(ConfirmRejection::PurposeMismatch));
    }
    let code_matches = constant_time_eq(given_code_hash, &challenge.code_hash);
    match decide_confirmation(&challenge, current_generation, code_matches, now) {
        ConfirmDecision::Reject(reason) => {
            Ok(ConfirmResult::Rejected(ConfirmRejection::Challenge(reason)))
        }
        ConfirmDecision::WrongCode {
            attempts,
            invalidate,
        } => {
            challenge.attempts = attempts;
            if invalidate {
                challenge.status = ChallengeStatus::Invalidated;
            }
            repository.save(executor, &challenge).await?;
            Ok(ConfirmResult::Rejected(ConfirmRejection::WrongCode {
                attempts,
                invalidated: invalidate,
            }))
        }
        ConfirmDecision::Accept => {
            challenge.status = ChallengeStatus::Confirmed;
            challenge.confirmed_at = Some(now);
            let proof_expires_at = if challenge.purpose == ChallengePurpose::Registration {
                let expires_at = (now + policy.proof_ttl).min(challenge.flow_expires_at);
                challenge.proof_hash = Some(new_proof_hash.to_string());
                challenge.proof_expires_at = Some(expires_at);
                Some(expires_at)
            } else {
                None
            };
            repository.save(executor, &challenge).await?;
            Ok(ConfirmResult::Confirmed {
                purpose: challenge.purpose,
                proof_expires_at,
            })
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProofVerification {
    /// 検証済み。`replayed` は再利用窓内の transport retry。gate の verdict は含まない。
    Verified {
        replayed: bool,
    },
    Rejected(ProofRejection),
}

/// proof の照合と原子的な消費。flow / email / generation に束縛された proof だけを受理し、
/// 初回は消費済みにする。同じ束縛の再利用窓内の再呼出しには同じ検証済み事実を返す。
pub async fn consume_registration_proof<R: RegistrationEmailChallengeRepository>(
    repository: &R,
    executor: &mut R::Connection,
    proof_hash: &str,
    binding: &ProofBinding,
    policy: &EmailChallengePolicy,
    now: OffsetDateTime,
) -> error_stack::Result<ProofVerification, KernelError> {
    let mut challenge = repository
        .find_by_proof_hash_for_update(executor, proof_hash)
        .await?;
    if let Some(found) = &challenge {
        let stored = found.proof_hash.as_deref().unwrap_or("");
        if !constant_time_eq(stored, proof_hash) {
            challenge = None;
        }
    }
    match decide_proof_use(challenge.as_ref(), binding, now, policy.proof_reuse_window) {
        ProofDecision::Reject(reason) => Ok(ProofVerification::Rejected(reason)),
        ProofDecision::Replay => Ok(ProofVerification::Verified { replayed: true }),
        ProofDecision::Consume => {
            if let Some(mut challenge) = challenge {
                challenge.consumed_at = Some(now);
                repository.save(executor, &challenge).await?;
            }
            Ok(ProofVerification::Verified { replayed: false })
        }
    }
}
