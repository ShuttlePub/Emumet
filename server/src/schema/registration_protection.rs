use application::service::registration_protection::{
    AdminVerifyReport, LastCheckView, ProtectionReadinessView, VerifyDisposition,
};
use kernel::prelude::entity::ProviderResult;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;

#[derive(Deserialize, ToSchema)]
pub struct CreateEmailChallengeRequest {
    /// 所有を確認したいメールアドレス (ASCII。trim・小文字化して扱う)。
    pub email: String,
    /// 進行中の registration flow の ID (Kratos の flow.id)。challenge はこの flow に束縛される。
    pub flow_id: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EmailChallengeResponse {
    /// code 確認 (`/confirm`) に使う opaque な challenge ID。
    pub challenge_id: String,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

#[derive(Deserialize, ToSchema)]
pub struct ConfirmEmailChallengeRequest {
    /// メールで届いた code。
    pub code: String,
}

#[derive(Serialize, ToSchema)]
pub struct EmailProofResponse {
    /// 登録時に `transient_payload.email_proof` として渡す opaque proof (一度だけ表示される)。
    pub proof: String,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LastCheckResponse {
    #[serde(with = "time::serde::rfc3339")]
    pub checked_at: OffsetDateTime,
    /// 最後の検証の business 結果: `business_success` / `accepted` (資格情報・接続は受理されたが
    /// business success ではない) / `configuration_invalid` / `temporarily_unavailable`。
    pub outcome: String,
    /// secret を含まない固定語彙の理由。
    pub reason: String,
    /// transport の分類 (HTTP status の種別・timeout・SMTP 応答の種別など)。HTTP 200 は business 結果と別。
    pub transport: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_success_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub success_valid_until: Option<OffsetDateTime>,
    /// 記録が現在の設定世代のものか。false なら設定変更後で、旧世代の成功は使われない。
    pub generation_matches: bool,
}

impl From<&LastCheckView> for LastCheckResponse {
    fn from(view: &LastCheckView) -> Self {
        Self {
            checked_at: view.checked_at,
            outcome: view.outcome.as_str().to_string(),
            reason: view.reason.as_str().to_string(),
            transport: view.transport.as_str().to_string(),
            last_success_at: view.last_success_at,
            success_valid_until: view.success_valid_until,
            generation_matches: view.generation_matches,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProtectionReadinessResponse {
    /// `email` / `bot`
    pub protection: String,
    /// `smtp` / `turnstile`
    pub provider: String,
    /// 設定が有効か (未設定・明示無効は false)。
    pub enabled: bool,
    /// 設定世代。設定 (provider・非 secret 設定値・secret の fingerprint) が変わると変わる。
    pub generation: String,
    /// secret の逆算できない短い fingerprint (secret 自体は出ない)。
    pub fingerprint: String,
    /// 現在時刻での評価: `configuration_invalid` / `operational` / `temporarily_unavailable` / `unverified`。
    pub status: String,
    /// 評価の理由 (secret を含まない固定語彙)。
    pub reason: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub checked_at: Option<OffsetDateTime>,
    /// operational が続く期限 (operational のときだけ)。超過すると unverified になる。
    #[serde(with = "time::serde::rfc3339::option")]
    pub valid_until: Option<OffsetDateTime>,
    pub last_check: Option<LastCheckResponse>,
}

impl From<&ProtectionReadinessView> for ProtectionReadinessResponse {
    fn from(view: &ProtectionReadinessView) -> Self {
        Self {
            protection: match view.protection {
                kernel::prelude::entity::Protection::Email => "email",
                kernel::prelude::entity::Protection::Bot => "bot",
            }
            .to_string(),
            provider: view.provider.as_str().to_string(),
            enabled: view.enabled,
            generation: view.generation.as_str().to_string(),
            fingerprint: view.fingerprint.clone(),
            status: view.effective.status.as_str().to_string(),
            reason: view.effective.reason.as_str().to_string(),
            checked_at: view.effective.checked_at,
            valid_until: view.effective.valid_until,
            last_check: view.last_check.as_ref().map(Into::into),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProtectionListResponse {
    pub protections: Vec<ProtectionReadinessResponse>,
}

#[derive(Default, Deserialize, ToSchema)]
pub struct VerifyProtectionRequest {
    /// bot: widget が発行した有効な token (business 検証)。省略すると secret の負の probe だけを行い、
    /// operational にはならない。
    #[serde(default)]
    pub token: Option<String>,
    /// email: 先に送った canary challenge の ID。`code` と一緒に指定すると roundtrip を確認する。
    #[serde(default)]
    pub challenge_id: Option<String>,
    /// email: 承認済み canary mailbox に届いた code。
    #[serde(default)]
    pub code: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProviderResultResponse {
    /// business 結果: `business_success` / `accepted` / `configuration_invalid` /
    /// `attempt_rejected` / `temporarily_unavailable`。
    pub outcome: String,
    pub reason: String,
    pub transport: String,
}

impl From<&ProviderResult> for ProviderResultResponse {
    fn from(result: &ProviderResult) -> Self {
        Self {
            outcome: result.outcome.as_str().to_string(),
            reason: result.reason.as_str().to_string(),
            transport: result.transport.as_str().to_string(),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PendingChallengeResponse {
    pub challenge_id: String,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct VerifyProtectionResponse {
    pub result: ProviderResultResponse,
    /// readiness へ記録したか。当該試行だけの拒否 (`attempt_rejected`) は記録しない。
    pub recorded: bool,
    /// `completed` / `pending_confirmation` (canary mailbox に code を送った) /
    /// `canary_not_configured` (承認済み canary mailbox が未設定で、メールは unverified のまま)。
    pub disposition: String,
    pub pending_challenge: Option<PendingChallengeResponse>,
    pub readiness: ProtectionReadinessResponse,
}

impl From<&AdminVerifyReport> for VerifyProtectionResponse {
    fn from(report: &AdminVerifyReport) -> Self {
        let (disposition, pending_challenge) = match &report.disposition {
            VerifyDisposition::Completed => ("completed", None),
            VerifyDisposition::CanaryNotConfigured => ("canary_not_configured", None),
            VerifyDisposition::PendingConfirmation {
                challenge_id,
                expires_at,
            } => (
                "pending_confirmation",
                Some(PendingChallengeResponse {
                    challenge_id: challenge_id.clone(),
                    expires_at: *expires_at,
                }),
            ),
        };
        Self {
            result: (&report.result).into(),
            recorded: report.recorded,
            disposition: disposition.to_string(),
            pending_challenge,
            readiness: (&report.readiness).into(),
        }
    }
}
