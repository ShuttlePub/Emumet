use application::service::registration_gate::{
    RegistrationGateNotifications, RegistrationGateView,
};
use kernel::prelude::entity::{
    AdminState, GateEventKind, HealthEpisode, ProbeStatus, Protection, ProtectionObservation,
    RegistrationDecision, RegistrationGateNotification,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;

#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct RegistrationGateRevisionRequest {
    /// 取得した状態の revision。一致しなければ 409。
    pub revision: i64,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct EnableOverrideRequest {
    /// 取得した状態の revision。一致しなければ 409。
    pub revision: i64,
    /// 保護が整っていない状態でも一般登録を開放する危険を理解したことの明示確認。false なら 400。
    #[serde(default)]
    pub acknowledge_warning: bool,
}

#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct MarkNotificationsReadRequest {
    /// 既読にする通知イベント ID (1〜100 件)。
    pub event_ids: Vec<String>,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct NotificationsQuery {
    /// 1〜100 (既定 50)。
    pub limit: Option<i64>,
    /// 前ページ最後の通知イベント ID。これより古いものを返す。
    pub before: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProtectionObservationResponse {
    pub configured: bool,
    /// `healthy` / `unhealthy` / `unknown`
    pub probe: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RegistrationObservationResponse {
    pub email: ProtectionObservationResponse,
    pub bot: ProtectionObservationResponse,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OverrideResponse {
    /// override 有効中は true。管理画面は全ページで常時警告表示する。
    pub active: bool,
    pub activated_by: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub activated_at: Option<OffsetDateTime>,
}

/// E15a は control plane のみ。Kratos 遮断 (E15d) が接続されるまで、この状態は実際の登録を
/// 遮断も許可もしない (実登録はまだ守られない)。
#[derive(Debug, Serialize, ToSchema)]
pub struct RegistrationGateResponse {
    pub revision: i64,
    /// `open` / `closed` (管理者の開閉状態)
    pub admin_state: String,
    /// `accepting` / `admin_closed` / `stopped` / `temporary_reject`
    pub decision: String,
    pub accepting: bool,
    /// `stopped` (設定喪失の停止ラッチ) のとき true。復旧しても自動再開せず手動再開が必要。
    pub requires_manual_resume: bool,
    pub config_lost_latch: bool,
    /// `none` / `outage` / `degraded`
    pub health_episode: String,
    /// 登録時に課す対策 (検証 API が実際に応答した対策のみ)
    pub enforced_protections: Vec<String>,
    /// 設定済みだが検証 API が応答しない対策 (防御低下)
    pub degraded_protections: Vec<String>,
    #[serde(rename = "override")]
    pub override_state: OverrideResponse,
    pub observation: RegistrationObservationResponse,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RegistrationGateNotificationResponse {
    /// 安定した通知イベント ID (外部 Webhook 配送の重複排除キーにもなる)。
    pub id: String,
    /// `outage_started` / `outage_recovered` / `protection_degraded` /
    /// `protection_lost_stopped` / `override_started` / `override_revoked`
    pub kind: String,
    /// このイベントを生んだ遷移後のゲート revision。
    pub revision: i64,
    /// override の開始・解除を実行した管理者 (auth account ID)。
    pub actor: Option<String>,
    pub observation: RegistrationObservationResponse,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// 呼び出した管理者自身の既読状態。
    pub read: bool,
    #[serde(with = "time::serde::rfc3339::option")]
    pub read_at: Option<OffsetDateTime>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct RegistrationGateNotificationListResponse {
    pub notifications: Vec<RegistrationGateNotificationResponse>,
    /// 呼び出した管理者自身の未読件数。
    pub unread_count: i64,
    /// 次ページを取るための `before` 値。最後のページでは null。
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct UnreadCountResponse {
    /// 呼び出した管理者自身の未読件数。
    pub unread_count: i64,
}

fn serde_str<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(value)) => value,
        _ => String::new(),
    }
}

fn protection_names(protections: &[Protection]) -> Vec<String> {
    protections.iter().map(serde_str).collect()
}

impl From<&ProtectionObservation> for ProtectionObservationResponse {
    fn from(value: &ProtectionObservation) -> Self {
        Self {
            configured: value.configured,
            probe: probe_value(&value.probe),
        }
    }
}

fn probe_value(probe: &ProbeStatus) -> String {
    serde_str(probe)
}

impl From<&kernel::prelude::entity::RegistrationObservation> for RegistrationObservationResponse {
    fn from(value: &kernel::prelude::entity::RegistrationObservation) -> Self {
        Self {
            email: (&value.email).into(),
            bot: (&value.bot).into(),
        }
    }
}

impl From<&RegistrationGateView> for RegistrationGateResponse {
    fn from(view: &RegistrationGateView) -> Self {
        let state = &view.state;
        let (decision, enforced, degraded) = match &view.decision {
            RegistrationDecision::Accept {
                enforced, degraded, ..
            } => ("accepting", enforced.as_slice(), degraded.as_slice()),
            RegistrationDecision::AdminClosed => ("admin_closed", &[][..], &[][..]),
            RegistrationDecision::Stopped => ("stopped", &[][..], &[][..]),
            RegistrationDecision::TemporaryReject => ("temporary_reject", &[][..], &[][..]),
        };
        Self {
            revision: state.revision,
            admin_state: admin_state_value(&state.admin),
            decision: decision.to_string(),
            accepting: view.decision.accepts(),
            requires_manual_resume: view.decision == RegistrationDecision::Stopped,
            config_lost_latch: state.config_lost_latch,
            health_episode: episode_value(&state.episode),
            enforced_protections: protection_names(enforced),
            degraded_protections: protection_names(degraded),
            override_state: OverrideResponse {
                active: state.is_override_active(),
                activated_by: state
                    .override_info
                    .as_ref()
                    .map(|info| info.activated_by.as_ref().to_string()),
                activated_at: state.override_info.as_ref().map(|info| info.activated_at),
            },
            observation: (&state.observation).into(),
            updated_at: state.updated_at,
        }
    }
}

fn admin_state_value(admin: &AdminState) -> String {
    serde_str(admin)
}

fn episode_value(episode: &HealthEpisode) -> String {
    serde_str(episode)
}

fn kind_value(kind: &GateEventKind) -> String {
    serde_str(kind)
}

impl From<&RegistrationGateNotification> for RegistrationGateNotificationResponse {
    fn from(notification: &RegistrationGateNotification) -> Self {
        let event = &notification.event;
        Self {
            id: event.id.as_ref().to_string(),
            kind: kind_value(&event.kind),
            revision: event.revision,
            actor: event.actor.as_ref().map(|actor| actor.as_ref().to_string()),
            observation: (&event.observation).into(),
            created_at: event.created_at,
            read: notification.read_at.is_some(),
            read_at: notification.read_at,
        }
    }
}

impl From<&RegistrationGateNotifications> for RegistrationGateNotificationListResponse {
    fn from(notifications: &RegistrationGateNotifications) -> Self {
        Self {
            notifications: notifications.notifications.iter().map(Into::into).collect(),
            unread_count: notifications.unread_count,
            next_cursor: notifications
                .next_cursor
                .as_ref()
                .map(|id| id.as_ref().to_string()),
        }
    }
}
