use crate::entity::{
    AuthAccountId, GateControlSnapshot, RegistrationGateState, RegistrationObservation,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use vodca::{AsRefln, Fromln, Newln};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Newln, Serialize, Deserialize)]
pub struct RegistrationGateEventId(i64);

impl Default for RegistrationGateEventId {
    fn default() -> Self {
        RegistrationGateEventId(crate::generate_id())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Newln, Serialize, Deserialize)]
pub struct RegistrationGateAuditId(i64);

impl Default for RegistrationGateAuditId {
    fn default() -> Self {
        RegistrationGateAuditId(crate::generate_id())
    }
}

/// 管理者通知イベントの種別。E15w が同じ永続イベントを外部 Webhook へ配送する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateEventKind {
    OutageStarted,
    OutageRecovered,
    ProtectionDegraded,
    ProtectionLostStopped,
    EnforcementLostStopped,
    EnforcementRestored,
    OverrideStarted,
    OverrideRevoked,
}

/// 状態遷移を引き起こした操作。`Observation` は内部入力による遷移 (actor なし)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateAction {
    Open,
    Close,
    EnableOverride,
    RevokeOverride,
    Observation,
}

/// 永続化する通知イベント。`revision` はこのイベントを生んだ遷移後の gate revision。
/// (revision, kind) は一意で、同じ遷移から同種のイベントが重複して生成されることはない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrationGateEvent {
    pub id: RegistrationGateEventId,
    pub kind: GateEventKind,
    pub revision: i64,
    pub actor: Option<AuthAccountId>,
    pub observation: RegistrationObservation,
    pub created_at: OffsetDateTime,
}

/// 管理者ごとの既読状態を付けた通知。`read_at` は呼び出した管理者自身の既読時刻。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationGateNotification {
    pub event: RegistrationGateEvent,
    pub read_at: Option<OffsetDateTime>,
}

/// 状態遷移 1 回につき 1 件の監査記録 (revision_after が一意)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrationGateAudit {
    pub id: RegistrationGateAuditId,
    pub action: GateAction,
    pub actor: Option<AuthAccountId>,
    pub warning_acknowledged: bool,
    pub before: GateControlSnapshot,
    pub after: GateControlSnapshot,
    pub observation: RegistrationObservation,
    pub created_at: OffsetDateTime,
}

/// 同一 transaction で保存する遷移一式 (新状態 + 監査 + 通知イベント)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateTransitionRecord {
    pub expected_revision: i64,
    pub state: RegistrationGateState,
    pub audit: RegistrationGateAudit,
    pub events: Vec<RegistrationGateEvent>,
}
