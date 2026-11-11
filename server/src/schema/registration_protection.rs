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

/// 入力なしの検証。本文は空のオブジェクト `{}` (未知のフィールドは 400)。
/// 旧 `token` / `challenge_id` / `code` は廃止された (d15-q05: 管理者の入力なしの再検証だけで準備済みになる)。
#[derive(Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct VerifyProtectionRequest {}

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

/// 互換のために残す形。canary の code 確認は廃止されたため、`pending_challenge` は常に `null`。
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
    /// `completed` / `canary_not_configured` (承認済み canary mailbox が未設定で、メールは unverified のまま)。
    /// 旧 `pending_confirmation` (code 確認待ち) は返さない。
    pub disposition: String,
    /// 互換のための項目。常に `null` (code 確認は廃止)。
    pub pending_challenge: Option<PendingChallengeResponse>,
    pub readiness: ProtectionReadinessResponse,
}

impl From<&AdminVerifyReport> for VerifyProtectionResponse {
    fn from(report: &AdminVerifyReport) -> Self {
        let disposition = match &report.disposition {
            VerifyDisposition::Completed => "completed",
            VerifyDisposition::CanaryNotConfigured => "canary_not_configured",
        };
        Self {
            result: (&report.result).into(),
            recorded: report.recorded,
            disposition: disposition.to_string(),
            pending_challenge: None,
            readiness: (&report.readiness).into(),
        }
    }
}
