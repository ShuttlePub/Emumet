//! 登録ゲートの純粋状態機械。状態表 (packet 記載) をここで一意に実装する。
//!
//! 判定ロジックの変更 (override の細部など) はこのファイルの `evaluate` / `apply_command` /
//! `apply_observation` / `reconcile_episode` だけで完結するように、I/O は一切持たない。

use crate::entity::{
    AdminState, AuthAccountId, GateAction, GateControlSnapshot, GateEventKind,
    GateTransitionRecord, HealthEpisode, OverrideInfo, RegistrationDecision, RegistrationGateAudit,
    RegistrationGateAuditId, RegistrationGateEvent, RegistrationGateEventId, RegistrationGateState,
    RegistrationObservation,
};
use crate::KernelError;
use error_stack::Report;
use time::OffsetDateTime;

/// 管理者が実行する遷移操作 (すべて revision 必須・監査対象)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateCommand {
    /// 最低条件 (effective ≥ 1) を満たす場合のみ成功し、ラッチを解除する (手動再開を兼ねる)。
    Open,
    /// 手動閉鎖。override / ラッチの値は保持するが判定は常に AdminClosed。
    Close,
    /// 警告確認 (ack) 必須。最低条件を満たさなくても開放でき、override を有効にする。
    EnableOverride { warning_acknowledged: bool },
    /// override を解除し、解除時点の観測で通常判定へ戻る。管理者状態は Closed にしない。
    RevokeOverride,
}

/// 観測値を除いた「制御状態」が変わった遷移 (監査・通知・revision +1 の対象)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateTransition {
    pub action: GateAction,
    pub actor: Option<AuthAccountId>,
    pub warning_acknowledged: bool,
    pub before: GateControlSnapshot,
    pub after: RegistrationGateState,
    pub events: Vec<GateEventKind>,
}

impl GateTransition {
    /// 永続化レコードへ変換する。監査 ID と通知イベント ID はここで採番する。
    pub fn into_record(self, now: OffsetDateTime) -> GateTransitionRecord {
        let expected_revision = self.before.revision;
        let audit = RegistrationGateAudit {
            id: RegistrationGateAuditId::default(),
            action: self.action,
            actor: self.actor.clone(),
            warning_acknowledged: self.warning_acknowledged,
            before: self.before,
            after: self.after.control_snapshot(),
            observation: self.after.observation,
            created_at: now,
        };
        let events = self
            .events
            .iter()
            .map(|kind| RegistrationGateEvent {
                id: RegistrationGateEventId::default(),
                kind: *kind,
                revision: self.after.revision,
                actor: match kind {
                    GateEventKind::OverrideStarted | GateEventKind::OverrideRevoked => {
                        self.actor.clone()
                    }
                    _ => None,
                },
                observation: self.after.observation,
                created_at: now,
            })
            .collect();
        GateTransitionRecord {
            expected_revision,
            state: self.after,
            audit,
            events,
        }
    }
}

/// 観測入力を反映した結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservationOutcome {
    /// 観測を反映した後の状態 (遷移があれば revision +1 済み)。
    pub state: RegistrationGateState,
    /// 制御状態が変わった場合のみ Some。
    pub transition: Option<GateTransition>,
    /// 観測値そのものが変わったか。遷移がなくても観測値は永続化する。
    pub observation_changed: bool,
}

/// 観測の分類 (状態表の観測列)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProtectionHealth {
    NoConfig,
    Outage,
    Degraded,
    Healthy,
}

fn classify(observation: &RegistrationObservation) -> ProtectionHealth {
    if observation.configured_count() == 0 {
        ProtectionHealth::NoConfig
    } else if !observation.meets_minimum() {
        ProtectionHealth::Outage
    } else if observation.degraded_protections().is_empty() {
        ProtectionHealth::Healthy
    } else {
        ProtectionHealth::Degraded
    }
}

/// 新規登録に対する判定 (状態表)。
pub fn evaluate(state: &RegistrationGateState) -> RegistrationDecision {
    if state.admin == AdminState::Closed {
        return RegistrationDecision::AdminClosed;
    }
    let observation = &state.observation;
    let accept = |override_active| RegistrationDecision::Accept {
        enforced: observation.effective_protections(),
        degraded: observation.degraded_protections(),
        override_active,
    };
    if state.is_override_active() {
        return accept(true);
    }
    if state.config_lost_latch {
        return RegistrationDecision::Stopped;
    }
    match classify(observation) {
        ProtectionHealth::NoConfig => RegistrationDecision::Stopped,
        ProtectionHealth::Outage => RegistrationDecision::TemporaryReject,
        ProtectionHealth::Degraded | ProtectionHealth::Healthy => accept(false),
    }
}

/// 通常判定の対象か (Open かつ override なし かつ ラッチなし)。
/// episode を新規に開始できるのはこの文脈のみ。
fn is_judged(state: &RegistrationGateState) -> bool {
    state.admin == AdminState::Open && !state.is_override_active() && !state.config_lost_latch
}

/// episode の遷移と通知イベントを決める。
///
/// - judged: 観測に応じて episode を開始・更新し、開始/復旧のイベントを出す
/// - not judged (Closed / override / ラッチ): 新規 episode は開始しない。継続中の Outage が
///   effective ≥ 1 に戻ったときだけ復旧イベントを出す。それ以外の episode は黙って終了する
///
/// 同一 episode が継続する間 (episode が変わらない観測の反復) はイベントを出さない。
fn reconcile_episode(
    previous: HealthEpisode,
    judged: bool,
    observation: &RegistrationObservation,
) -> (HealthEpisode, Vec<GateEventKind>) {
    let health = classify(observation);
    let recovered = previous == HealthEpisode::Outage
        && matches!(
            health,
            ProtectionHealth::Degraded | ProtectionHealth::Healthy
        );
    let mut events = Vec::new();
    if recovered {
        events.push(GateEventKind::OutageRecovered);
    }
    let next = match (judged, health) {
        (true, ProtectionHealth::Outage) => {
            if previous != HealthEpisode::Outage {
                events.push(GateEventKind::OutageStarted);
            }
            HealthEpisode::Outage
        }
        (true, ProtectionHealth::Degraded) => {
            if previous != HealthEpisode::Degraded {
                events.push(GateEventKind::ProtectionDegraded);
            }
            HealthEpisode::Degraded
        }
        (false, ProtectionHealth::Outage) if previous == HealthEpisode::Outage => {
            HealthEpisode::Outage
        }
        (false, ProtectionHealth::Degraded) if previous == HealthEpisode::Degraded => {
            HealthEpisode::Degraded
        }
        _ => HealthEpisode::None,
    };
    (next, events)
}

/// Open かつ override Off で configured が 0 件のときに停止ラッチを成立させる (q02)。
fn latch_on_config_lost(
    state: &mut RegistrationGateState,
    events: &mut Vec<GateEventKind>,
) -> bool {
    if is_judged(state) && classify(&state.observation) == ProtectionHealth::NoConfig {
        state.config_lost_latch = true;
        state.episode = HealthEpisode::None;
        events.push(GateEventKind::ProtectionLostStopped);
        true
    } else {
        false
    }
}

fn same_control(a: &RegistrationGateState, b: &RegistrationGateState) -> bool {
    a.admin == b.admin
        && a.override_info == b.override_info
        && a.config_lost_latch == b.config_lost_latch
        && a.episode == b.episode
}

/// 管理者操作を適用する。revision 不一致は `Concurrency`、状態表に反する操作は `Rejected`、
/// override の警告確認なしは `Validation`。
pub fn apply_command(
    state: &RegistrationGateState,
    expected_revision: i64,
    command: GateCommand,
    actor: &AuthAccountId,
    now: OffsetDateTime,
) -> error_stack::Result<GateTransition, KernelError> {
    if state.revision != expected_revision {
        return Err(
            Report::new(KernelError::Concurrency).attach_printable(format!(
                "Registration gate revision mismatch: expected {expected_revision}, actual {}",
                state.revision
            )),
        );
    }
    let before = state.control_snapshot();
    let mut next = state.clone();
    let mut events = Vec::new();
    let mut warning_acknowledged = false;

    let action = match command {
        GateCommand::Open => {
            if state.admin == AdminState::Open && !state.config_lost_latch {
                return Err(Report::new(KernelError::Rejected)
                    .attach_printable("Registration gate is already open"));
            }
            if !state.observation.meets_minimum() {
                return Err(Report::new(KernelError::Rejected).attach_printable(
                    "Minimum protection condition (email OR bot) is not met; \
                     use override with warning acknowledgement to open anyway",
                ));
            }
            next.admin = AdminState::Open;
            next.config_lost_latch = false;
            let (episode, episode_events) =
                reconcile_episode(next.episode, is_judged(&next), &next.observation);
            next.episode = episode;
            events.extend(episode_events);
            GateAction::Open
        }
        GateCommand::Close => {
            if state.admin == AdminState::Closed {
                return Err(Report::new(KernelError::Rejected)
                    .attach_printable("Registration gate is already closed"));
            }
            next.admin = AdminState::Closed;
            GateAction::Close
        }
        GateCommand::EnableOverride {
            warning_acknowledged: acknowledged,
        } => {
            if !acknowledged {
                return Err(Report::new(KernelError::Validation).attach_printable(
                    "Opening without protection requires acknowledging the warning".to_string(),
                ));
            }
            if state.is_override_active() && state.admin == AdminState::Open {
                return Err(Report::new(KernelError::Rejected)
                    .attach_printable("Override is already active"));
            }
            warning_acknowledged = true;
            next.admin = AdminState::Open;
            next.config_lost_latch = false;
            if !state.is_override_active() {
                next.override_info = Some(OverrideInfo {
                    activated_by: actor.clone(),
                    activated_at: now,
                });
                next.episode = HealthEpisode::None;
                events.push(GateEventKind::OverrideStarted);
            }
            GateAction::EnableOverride
        }
        GateCommand::RevokeOverride => {
            if !state.is_override_active() {
                return Err(
                    Report::new(KernelError::Rejected).attach_printable("Override is not active")
                );
            }
            next.override_info = None;
            events.push(GateEventKind::OverrideRevoked);
            if !latch_on_config_lost(&mut next, &mut events) {
                let (episode, episode_events) =
                    reconcile_episode(next.episode, is_judged(&next), &next.observation);
                next.episode = episode;
                events.extend(episode_events);
            }
            GateAction::RevokeOverride
        }
    };

    next.revision = state.revision + 1;
    next.updated_at = now;
    Ok(GateTransition {
        action,
        actor: Some(actor.clone()),
        warning_acknowledged,
        before,
        after: next,
        events,
    })
}

/// 観測入力を反映して判定を再計算する。
///
/// 制御状態 (admin / override / ラッチ / episode) が変わらない観測の反復は遷移にならず、
/// revision・監査・通知のいずれも増やさない。
pub fn apply_observation(
    state: &RegistrationGateState,
    observation: RegistrationObservation,
    now: OffsetDateTime,
) -> ObservationOutcome {
    let mut next = state.clone();
    next.observation = observation;
    let observation_changed = state.observation != observation;

    let mut events = Vec::new();
    if !latch_on_config_lost(&mut next, &mut events) {
        let (episode, episode_events) =
            reconcile_episode(state.episode, is_judged(state), &observation);
        next.episode = episode;
        events.extend(episode_events);
    }

    if same_control(state, &next) {
        if observation_changed {
            next.updated_at = now;
        }
        return ObservationOutcome {
            state: next,
            transition: None,
            observation_changed,
        };
    }

    next.revision = state.revision + 1;
    next.updated_at = now;
    let transition = GateTransition {
        action: GateAction::Observation,
        actor: None,
        warning_acknowledged: false,
        before: state.control_snapshot(),
        after: next.clone(),
        events,
    };
    ObservationOutcome {
        state: next,
        transition: Some(transition),
        observation_changed,
    }
}

#[cfg(test)]
mod tests;
