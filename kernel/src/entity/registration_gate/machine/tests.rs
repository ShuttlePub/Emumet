use super::*;
use crate::entity::{ProbeStatus, Protection, ProtectionObservation};
use time::OffsetDateTime;

mod enforcement;

fn now() -> OffsetDateTime {
    OffsetDateTime::UNIX_EPOCH
}

fn admin_id() -> AuthAccountId {
    AuthAccountId::new(1)
}

fn other_admin_id() -> AuthAccountId {
    AuthAccountId::new(2)
}

const H: ProtectionObservation = ProtectionObservation {
    configured: true,
    probe: ProbeStatus::Healthy,
};
const UNHEALTHY: ProtectionObservation = ProtectionObservation {
    configured: true,
    probe: ProbeStatus::Unhealthy,
};
const UNKNOWN: ProtectionObservation = ProtectionObservation {
    configured: true,
    probe: ProbeStatus::Unknown,
};
const OFF: ProtectionObservation = ProtectionObservation::UNCONFIGURED;

fn obs(email: ProtectionObservation, bot: ProtectionObservation) -> RegistrationObservation {
    RegistrationObservation::new(email, bot)
}

fn state(
    admin: AdminState,
    override_on: bool,
    latch: bool,
    episode: HealthEpisode,
    observation: RegistrationObservation,
) -> RegistrationGateState {
    RegistrationGateState {
        admin,
        override_info: override_on.then(|| OverrideInfo {
            activated_by: admin_id(),
            activated_at: now(),
        }),
        config_lost_latch: latch,
        enforcement_lost_latch: false,
        episode,
        observation,
        revision: 7,
        updated_at: now(),
    }
}

fn open_state(observation: RegistrationObservation) -> RegistrationGateState {
    state(
        AdminState::Open,
        false,
        false,
        HealthEpisode::None,
        observation,
    )
}

fn command(
    state: &RegistrationGateState,
    command: GateCommand,
) -> error_stack::Result<GateTransition, KernelError> {
    apply_command(state, state.revision, command, &admin_id(), now())
}

fn run_command(state: &RegistrationGateState, cmd: GateCommand) -> GateTransition {
    command(state, cmd).expect("command should succeed")
}

fn kind_of(result: error_stack::Result<GateTransition, KernelError>) -> KernelError {
    match result {
        Ok(_) => panic!("expected error"),
        Err(report) => match report.current_context() {
            KernelError::Concurrency => KernelError::Concurrency,
            KernelError::Rejected => KernelError::Rejected,
            KernelError::Validation => KernelError::Validation,
            other => panic!("unexpected error {other:?}"),
        },
    }
}

fn observe(
    state: &RegistrationGateState,
    observation: RegistrationObservation,
) -> (RegistrationGateState, Vec<GateEventKind>) {
    let outcome = apply_observation(state, observation, now());
    let events = outcome
        .transition
        .as_ref()
        .map(|t| t.events.clone())
        .unwrap_or_default();
    (outcome.state, events)
}

fn accepting(state: &RegistrationGateState) -> bool {
    evaluate(state).accepts()
}

const ACK: GateCommand = GateCommand::EnableOverride {
    warning_acknowledged: true,
};

#[test]
fn minimum_condition_is_email_or_bot() {
    assert!(obs(H, OFF).meets_minimum(), "email only");
    assert!(obs(OFF, H).meets_minimum(), "bot only");
    assert!(obs(H, H).meets_minimum(), "both");
    assert!(!obs(OFF, OFF).meets_minimum(), "none");
}

#[test]
fn minimum_condition_requires_a_healthy_observation_not_just_configuration() {
    assert!(!obs(UNKNOWN, OFF).meets_minimum());
    assert!(!obs(UNHEALTHY, OFF).meets_minimum());
    assert!(!obs(UNKNOWN, UNHEALTHY).meets_minimum());
    assert!(obs(UNHEALTHY, H).meets_minimum());
}

#[test]
fn unconfigured_protection_is_never_effective_even_if_probe_says_healthy() {
    let ghost = ProtectionObservation::new(false, ProbeStatus::Healthy);
    assert!(!ghost.is_effective());
    assert!(!obs(ghost, ghost).meets_minimum());
}

fn protection_observations() -> Vec<ProtectionObservation> {
    vec![
        OFF,
        ProtectionObservation::new(false, ProbeStatus::Healthy),
        H,
        UNHEALTHY,
        UNKNOWN,
    ]
}

#[test]
fn closed_is_always_admin_closed_regardless_of_override_latch_or_observation() {
    for email in protection_observations() {
        for bot in protection_observations() {
            for override_on in [false, true] {
                for latch in [false, true] {
                    let s = state(
                        AdminState::Closed,
                        override_on,
                        latch,
                        HealthEpisode::None,
                        obs(email, bot),
                    );
                    assert_eq!(evaluate(&s), RegistrationDecision::AdminClosed);
                }
            }
        }
    }
}

#[test]
fn open_with_override_accepts_for_every_observation_and_enforces_only_effective_protections() {
    for email in protection_observations() {
        for bot in protection_observations() {
            let observation = obs(email, bot);
            let s = state(
                AdminState::Open,
                true,
                false,
                HealthEpisode::None,
                observation,
            );
            assert_eq!(
                evaluate(&s),
                RegistrationDecision::Accept {
                    enforced: observation.effective_protections(),
                    degraded: observation.degraded_protections(),
                    override_active: true,
                }
            );
        }
    }
}

#[test]
fn override_keeps_enforcing_ready_protection() {
    let s = state(
        AdminState::Open,
        true,
        false,
        HealthEpisode::None,
        obs(H, UNHEALTHY),
    );
    assert_eq!(
        evaluate(&s),
        RegistrationDecision::Accept {
            enforced: vec![Protection::Email],
            degraded: vec![Protection::Bot],
            override_active: true,
        }
    );
}

#[test]
fn override_without_any_ready_protection_enforces_nothing() {
    let s = state(
        AdminState::Open,
        true,
        false,
        HealthEpisode::None,
        obs(OFF, OFF),
    );
    assert_eq!(
        evaluate(&s),
        RegistrationDecision::Accept {
            enforced: vec![],
            degraded: vec![],
            override_active: true,
        }
    );
}

#[test]
fn open_without_override_and_latched_is_stopped_even_if_protection_recovered() {
    let s = state(
        AdminState::Open,
        false,
        true,
        HealthEpisode::None,
        obs(H, H),
    );
    assert_eq!(evaluate(&s), RegistrationDecision::Stopped);
}

#[test]
fn open_without_override_and_nothing_configured_is_stopped() {
    assert_eq!(
        evaluate(&open_state(obs(OFF, OFF))),
        RegistrationDecision::Stopped
    );
}

#[test]
fn open_without_override_and_no_effective_protection_is_temporary_reject() {
    for observation in [
        obs(UNHEALTHY, OFF),
        obs(OFF, UNHEALTHY),
        obs(UNKNOWN, OFF),
        obs(UNHEALTHY, UNHEALTHY),
        obs(UNHEALTHY, UNKNOWN),
    ] {
        assert_eq!(
            evaluate(&open_state(observation)),
            RegistrationDecision::TemporaryReject,
            "{observation:?}"
        );
    }
}

#[test]
fn open_without_override_with_effective_protection_accepts_and_enforces_effective_only() {
    assert_eq!(
        evaluate(&open_state(obs(H, OFF))),
        RegistrationDecision::Accept {
            enforced: vec![Protection::Email],
            degraded: vec![],
            override_active: false,
        }
    );
    assert_eq!(
        evaluate(&open_state(obs(OFF, H))),
        RegistrationDecision::Accept {
            enforced: vec![Protection::Bot],
            degraded: vec![],
            override_active: false,
        }
    );
    assert_eq!(
        evaluate(&open_state(obs(H, H))),
        RegistrationDecision::Accept {
            enforced: vec![Protection::Email, Protection::Bot],
            degraded: vec![],
            override_active: false,
        }
    );
}

#[test]
fn one_sided_failure_continues_with_only_the_working_protection() {
    assert_eq!(
        evaluate(&open_state(obs(UNHEALTHY, H))),
        RegistrationDecision::Accept {
            enforced: vec![Protection::Bot],
            degraded: vec![Protection::Email],
            override_active: false,
        }
    );
    assert_eq!(
        evaluate(&open_state(obs(H, UNKNOWN))),
        RegistrationDecision::Accept {
            enforced: vec![Protection::Email],
            degraded: vec![Protection::Bot],
            override_active: false,
        }
    );
}

#[test]
fn initial_state_is_closed() {
    let s = RegistrationGateState::initial(now());
    assert_eq!(s.admin, AdminState::Closed);
    assert_eq!(evaluate(&s), RegistrationDecision::AdminClosed);
    assert_eq!(s.revision, 0);
}

#[test]
fn open_succeeds_only_when_minimum_condition_is_met() {
    let closed = |observation| {
        state(
            AdminState::Closed,
            false,
            false,
            HealthEpisode::None,
            observation,
        )
    };
    for observation in [obs(H, OFF), obs(OFF, H), obs(H, H), obs(H, UNHEALTHY)] {
        let t = run_command(&closed(observation), GateCommand::Open);
        assert_eq!(t.after.admin, AdminState::Open);
        assert_eq!(t.after.revision, 8);
        assert!(accepting(&t.after));
    }
    for observation in [obs(OFF, OFF), obs(UNHEALTHY, OFF), obs(UNKNOWN, UNHEALTHY)] {
        assert_eq!(
            kind_of(command(&closed(observation), GateCommand::Open)),
            KernelError::Rejected,
            "{observation:?}"
        );
    }
}

#[test]
fn open_is_manual_resume_that_clears_the_latch() {
    let latched = state(
        AdminState::Open,
        false,
        true,
        HealthEpisode::None,
        obs(H, OFF),
    );
    assert_eq!(evaluate(&latched), RegistrationDecision::Stopped);

    let t = run_command(&latched, GateCommand::Open);
    assert!(!t.after.config_lost_latch);
    assert!(accepting(&t.after));
    assert_eq!(t.action, GateAction::Open);
}

#[test]
fn resuming_a_latched_gate_is_rejected_while_minimum_condition_is_unmet() {
    let latched = state(
        AdminState::Open,
        false,
        true,
        HealthEpisode::None,
        obs(UNHEALTHY, OFF),
    );
    assert_eq!(
        kind_of(command(&latched, GateCommand::Open)),
        KernelError::Rejected
    );
}

#[test]
fn open_when_already_open_and_not_latched_is_rejected() {
    assert_eq!(
        kind_of(command(&open_state(obs(H, OFF)), GateCommand::Open)),
        KernelError::Rejected
    );
}

#[test]
fn close_is_manual_and_keeps_latch_and_override_values() {
    let latched = state(
        AdminState::Open,
        false,
        true,
        HealthEpisode::None,
        obs(OFF, OFF),
    );
    let t = run_command(&latched, GateCommand::Close);
    assert_eq!(t.after.admin, AdminState::Closed);
    assert!(t.after.config_lost_latch);
    assert_eq!(evaluate(&t.after), RegistrationDecision::AdminClosed);

    let overridden = state(
        AdminState::Open,
        true,
        false,
        HealthEpisode::None,
        obs(OFF, OFF),
    );
    let t = run_command(&overridden, GateCommand::Close);
    assert!(t.after.is_override_active());
    assert_eq!(evaluate(&t.after), RegistrationDecision::AdminClosed);
}

#[test]
fn close_when_already_closed_is_rejected() {
    let closed = RegistrationGateState::initial(now());
    assert_eq!(
        kind_of(command(&closed, GateCommand::Close)),
        KernelError::Rejected
    );
}

#[test]
fn override_requires_warning_acknowledgement() {
    let s = RegistrationGateState::initial(now());
    assert_eq!(
        kind_of(command(
            &s,
            GateCommand::EnableOverride {
                warning_acknowledged: false
            }
        )),
        KernelError::Validation
    );
}

#[test]
fn override_opens_without_any_protection_and_emits_override_started() {
    let s = RegistrationGateState::initial(now());
    let t = run_command(&s, ACK);
    assert_eq!(t.after.admin, AdminState::Open);
    assert!(t.after.is_override_active());
    assert_eq!(
        t.after.override_info.as_ref().unwrap().activated_by,
        admin_id()
    );
    assert_eq!(t.events, vec![GateEventKind::OverrideStarted]);
    assert!(t.warning_acknowledged);
    assert!(accepting(&t.after));
}

#[test]
fn override_clears_latch_and_ends_episode_silently() {
    let latched = state(
        AdminState::Open,
        false,
        true,
        HealthEpisode::None,
        obs(OFF, OFF),
    );
    let t = run_command(&latched, ACK);
    assert!(!t.after.config_lost_latch);

    let outage = state(
        AdminState::Open,
        false,
        false,
        HealthEpisode::Outage,
        obs(UNHEALTHY, OFF),
    );
    let t = run_command(&outage, ACK);
    assert_eq!(t.after.episode, HealthEpisode::None);
    assert_eq!(t.events, vec![GateEventKind::OverrideStarted]);
}

#[test]
fn override_when_already_active_and_open_is_rejected() {
    let s = state(
        AdminState::Open,
        true,
        false,
        HealthEpisode::None,
        obs(OFF, OFF),
    );
    assert_eq!(kind_of(command(&s, ACK)), KernelError::Rejected);
}

#[test]
fn override_command_reopens_a_closed_gate_that_still_has_override_without_a_new_event() {
    let s = state(
        AdminState::Closed,
        true,
        false,
        HealthEpisode::None,
        obs(OFF, OFF),
    );
    let t = run_command(&s, ACK);
    assert_eq!(t.after.admin, AdminState::Open);
    assert!(t.events.is_empty());
    assert_eq!(
        t.after.override_info, s.override_info,
        "original activation info is kept"
    );
}

#[test]
fn plain_open_while_closed_with_override_still_requires_minimum_condition() {
    let s = state(
        AdminState::Closed,
        true,
        false,
        HealthEpisode::None,
        obs(OFF, OFF),
    );
    assert_eq!(
        kind_of(command(&s, GateCommand::Open)),
        KernelError::Rejected
    );
}

#[test]
fn revoke_override_requires_active_override() {
    assert_eq!(
        kind_of(command(
            &open_state(obs(H, OFF)),
            GateCommand::RevokeOverride
        )),
        KernelError::Rejected
    );
}

#[test]
fn revoke_override_reevaluates_against_the_current_observation() {
    let overridden = |observation| {
        state(
            AdminState::Open,
            true,
            false,
            HealthEpisode::None,
            observation,
        )
    };

    let t = run_command(&overridden(obs(OFF, OFF)), GateCommand::RevokeOverride);
    assert!(t.after.config_lost_latch);
    assert_eq!(
        t.after.admin,
        AdminState::Open,
        "revoke never closes the gate"
    );
    assert_eq!(evaluate(&t.after), RegistrationDecision::Stopped);
    assert_eq!(
        t.events,
        vec![
            GateEventKind::OverrideRevoked,
            GateEventKind::ProtectionLostStopped
        ]
    );

    let t = run_command(
        &overridden(obs(UNHEALTHY, OFF)),
        GateCommand::RevokeOverride,
    );
    assert_eq!(evaluate(&t.after), RegistrationDecision::TemporaryReject);
    assert_eq!(t.after.episode, HealthEpisode::Outage);
    assert_eq!(
        t.events,
        vec![GateEventKind::OverrideRevoked, GateEventKind::OutageStarted]
    );

    let t = run_command(&overridden(obs(H, OFF)), GateCommand::RevokeOverride);
    assert!(accepting(&t.after));
    assert_eq!(t.events, vec![GateEventKind::OverrideRevoked]);

    let t = run_command(&overridden(obs(H, UNHEALTHY)), GateCommand::RevokeOverride);
    assert!(accepting(&t.after));
    assert_eq!(t.after.episode, HealthEpisode::Degraded);
    assert_eq!(
        t.events,
        vec![
            GateEventKind::OverrideRevoked,
            GateEventKind::ProtectionDegraded
        ]
    );
}

#[test]
fn revoke_override_on_a_closed_gate_keeps_it_closed_and_does_not_latch() {
    let s = state(
        AdminState::Closed,
        true,
        false,
        HealthEpisode::None,
        obs(OFF, OFF),
    );
    let t = run_command(&s, GateCommand::RevokeOverride);
    assert_eq!(t.after.admin, AdminState::Closed);
    assert!(!t.after.config_lost_latch);
    assert_eq!(t.events, vec![GateEventKind::OverrideRevoked]);
}

#[test]
fn stale_revision_is_a_concurrency_error_for_every_command() {
    let s = open_state(obs(H, OFF));
    for cmd in [
        GateCommand::Open,
        GateCommand::Close,
        ACK,
        GateCommand::RevokeOverride,
    ] {
        let result = apply_command(&s, s.revision - 1, cmd, &admin_id(), now());
        assert_eq!(kind_of(result), KernelError::Concurrency);
    }
}

#[test]
fn override_events_carry_the_actor_in_the_record() {
    crate::ensure_generator_initialized();
    let s = RegistrationGateState::initial(now());
    let t = apply_command(&s, 0, ACK, &other_admin_id(), now()).unwrap();
    let record = t.into_record(now());
    assert_eq!(record.expected_revision, 0);
    assert_eq!(record.state.revision, 1);
    assert_eq!(record.audit.actor, Some(other_admin_id()));
    assert_eq!(record.audit.action, GateAction::EnableOverride);
    assert!(record.audit.warning_acknowledged);
    assert_eq!(record.events.len(), 1);
    assert_eq!(record.events[0].actor, Some(other_admin_id()));
    assert_eq!(record.events[0].revision, 1);
}

#[test]
fn one_sided_failure_emits_protection_degraded_exactly_once() {
    let healthy = open_state(obs(H, H));
    let (s1, e1) = observe(&healthy, obs(UNHEALTHY, H));
    assert_eq!(e1, vec![GateEventKind::ProtectionDegraded]);
    assert!(accepting(&s1));
    assert_eq!(s1.revision, healthy.revision + 1);

    let mut s = s1.clone();
    for _ in 0..5 {
        let outcome = apply_observation(&s, obs(UNHEALTHY, H), now());
        assert!(
            outcome.transition.is_none(),
            "probe repetition is not a transition"
        );
        assert!(!outcome.observation_changed);
        assert_eq!(outcome.state.revision, s1.revision);
        s = outcome.state;
    }
}

#[test]
fn degraded_flapping_within_one_episode_does_not_multiply_events() {
    let healthy = open_state(obs(H, H));
    let (s1, e1) = observe(&healthy, obs(UNHEALTHY, H));
    assert_eq!(e1.len(), 1);
    let (s2, e2) = observe(&s1, obs(UNKNOWN, H));
    assert!(e2.is_empty());
    assert_eq!(
        s2.revision, s1.revision,
        "revision follows control state only"
    );
    assert_eq!(s2.observation, obs(UNKNOWN, H));
}

#[test]
fn degraded_episode_that_ends_and_recurs_emits_a_new_event() {
    let healthy = open_state(obs(H, H));
    let (s1, e1) = observe(&healthy, obs(UNHEALTHY, H));
    assert_eq!(e1, vec![GateEventKind::ProtectionDegraded]);
    let (s2, e2) = observe(&s1, obs(H, H));
    assert!(e2.is_empty(), "no restore event type; episode just ends");
    assert_eq!(s2.episode, HealthEpisode::None);
    let (_, e3) = observe(&s2, obs(H, UNHEALTHY));
    assert_eq!(e3, vec![GateEventKind::ProtectionDegraded]);
}

#[test]
fn both_temporarily_down_rejects_then_recovery_resumes_automatically() {
    let healthy = open_state(obs(H, H));
    let (down, e1) = observe(&healthy, obs(UNHEALTHY, UNHEALTHY));
    assert_eq!(e1, vec![GateEventKind::OutageStarted]);
    assert_eq!(evaluate(&down), RegistrationDecision::TemporaryReject);
    assert!(!down.config_lost_latch);

    let (still_down, e2) = observe(&down, obs(UNHEALTHY, UNKNOWN));
    assert!(e2.is_empty());
    assert_eq!(evaluate(&still_down), RegistrationDecision::TemporaryReject);

    let (recovered, e3) = observe(&still_down, obs(UNHEALTHY, H));
    assert_eq!(
        e3,
        vec![
            GateEventKind::OutageRecovered,
            GateEventKind::ProtectionDegraded
        ]
    );
    assert!(
        accepting(&recovered),
        "auto resume without manual intervention"
    );

    let (full, e4) = observe(&recovered, obs(H, H));
    assert!(e4.is_empty());
    assert!(accepting(&full));
}

#[test]
fn full_recovery_from_outage_emits_only_recovery() {
    let down = state(
        AdminState::Open,
        false,
        false,
        HealthEpisode::Outage,
        obs(UNHEALTHY, UNHEALTHY),
    );
    let (s, e) = observe(&down, obs(H, H));
    assert_eq!(e, vec![GateEventKind::OutageRecovered]);
    assert!(accepting(&s));
}

#[test]
fn outage_that_recurs_after_recovery_is_a_new_episode() {
    let healthy = open_state(obs(H, OFF));
    let (down, e1) = observe(&healthy, obs(UNHEALTHY, OFF));
    let (up, e2) = observe(&down, obs(H, OFF));
    let (_, e3) = observe(&up, obs(UNHEALTHY, OFF));
    assert_eq!(e1, vec![GateEventKind::OutageStarted]);
    assert_eq!(e2, vec![GateEventKind::OutageRecovered]);
    assert_eq!(e3, vec![GateEventKind::OutageStarted]);
}

#[test]
fn config_lost_latches_stop_and_recovery_alone_never_resumes() {
    let healthy = open_state(obs(H, OFF));
    let (lost, e1) = observe(&healthy, obs(OFF, OFF));
    assert_eq!(e1, vec![GateEventKind::ProtectionLostStopped]);
    assert!(lost.config_lost_latch);
    assert_eq!(evaluate(&lost), RegistrationDecision::Stopped);

    let (recovered, e2) = observe(&lost, obs(H, H));
    assert!(e2.is_empty());
    assert!(recovered.config_lost_latch, "latch stays after recovery");
    assert_eq!(evaluate(&recovered), RegistrationDecision::Stopped);

    for _ in 0..3 {
        let (again, events) = observe(&recovered, obs(H, H));
        assert!(events.is_empty());
        assert_eq!(evaluate(&again), RegistrationDecision::Stopped);
    }
}

#[test]
fn config_lost_while_one_sided_degraded_latches_without_extra_events() {
    let degraded = state(
        AdminState::Open,
        false,
        false,
        HealthEpisode::Degraded,
        obs(UNHEALTHY, H),
    );
    let (lost, events) = observe(&degraded, obs(OFF, OFF));
    assert_eq!(events, vec![GateEventKind::ProtectionLostStopped]);
    assert_eq!(lost.episode, HealthEpisode::None);
}

#[test]
fn config_lost_during_outage_latches() {
    let down = state(
        AdminState::Open,
        false,
        false,
        HealthEpisode::Outage,
        obs(UNHEALTHY, OFF),
    );
    let (lost, events) = observe(&down, obs(OFF, OFF));
    assert!(lost.config_lost_latch);
    assert_eq!(events, vec![GateEventKind::ProtectionLostStopped]);
}

#[test]
fn only_one_protection_lost_does_not_latch() {
    let healthy = open_state(obs(H, H));
    let (s, events) = observe(&healthy, obs(OFF, H));
    assert!(!s.config_lost_latch);
    assert!(events.is_empty());
    assert!(accepting(&s));
}

#[test]
fn manual_resume_after_config_loss_requires_minimum_condition() {
    let healthy = open_state(obs(H, OFF));
    let (lost, _) = observe(&healthy, obs(OFF, OFF));
    assert_eq!(
        kind_of(command(&lost, GateCommand::Open)),
        KernelError::Rejected
    );
    let (recovered, _) = observe(&lost, obs(H, OFF));
    assert_eq!(evaluate(&recovered), RegistrationDecision::Stopped);
    let resumed = run_command(&recovered, GateCommand::Open);
    assert!(accepting(&resumed.after));
    assert!(!resumed.after.config_lost_latch);
}

#[test]
fn config_loss_while_closed_does_not_latch_or_notify() {
    let closed = state(
        AdminState::Closed,
        false,
        false,
        HealthEpisode::None,
        obs(H, OFF),
    );
    let (s, events) = observe(&closed, obs(OFF, OFF));
    assert!(!s.config_lost_latch);
    assert!(events.is_empty());
    assert_eq!(s.revision, closed.revision);
}

#[test]
fn manual_close_is_never_reopened_by_recovery() {
    let open = open_state(obs(H, OFF));
    let closed = run_command(&open, GateCommand::Close).after;
    for observation in [obs(UNHEALTHY, OFF), obs(H, H), obs(OFF, OFF), obs(H, OFF)] {
        let (s, _) = observe(&closed, observation);
        assert_eq!(evaluate(&s), RegistrationDecision::AdminClosed);
        assert_eq!(s.admin, AdminState::Closed);
    }
}

#[test]
fn outage_that_started_while_open_still_reports_recovery_after_manual_close() {
    let healthy = open_state(obs(H, OFF));
    let (down, started) = observe(&healthy, obs(UNHEALTHY, OFF));
    assert_eq!(started, vec![GateEventKind::OutageStarted]);
    let closed = run_command(&down, GateCommand::Close).after;
    assert_eq!(closed.episode, HealthEpisode::Outage);

    let (up, events) = observe(&closed, obs(H, OFF));
    assert_eq!(events, vec![GateEventKind::OutageRecovered]);
    assert_eq!(evaluate(&up), RegistrationDecision::AdminClosed);
}

#[test]
fn no_new_episode_starts_while_closed() {
    let closed = state(
        AdminState::Closed,
        false,
        false,
        HealthEpisode::None,
        obs(H, H),
    );
    let (s, events) = observe(&closed, obs(UNHEALTHY, UNHEALTHY));
    assert!(events.is_empty());
    assert_eq!(s.episode, HealthEpisode::None);
    assert_eq!(s.revision, closed.revision);
}

#[test]
fn override_suppresses_automatic_stop_and_outage_judgement() {
    let overridden = state(
        AdminState::Open,
        true,
        false,
        HealthEpisode::None,
        obs(H, H),
    );
    for observation in [obs(OFF, OFF), obs(UNHEALTHY, UNHEALTHY), obs(H, UNKNOWN)] {
        let outcome = apply_observation(&overridden, observation, now());
        assert!(outcome.transition.is_none(), "{observation:?}");
        assert!(!outcome.state.config_lost_latch);
        assert!(accepting(&outcome.state));
    }
}

#[test]
fn override_lifecycle_returns_to_normal_judgement_after_revoke() {
    let initial = RegistrationGateState::initial(now());
    let enabled = run_command(&initial, ACK).after;
    let (still, events) = observe(&enabled, obs(OFF, OFF));
    assert!(events.is_empty());
    assert!(accepting(&still));

    let revoked = run_command(&still, GateCommand::RevokeOverride);
    assert_eq!(evaluate(&revoked.after), RegistrationDecision::Stopped);
    assert_eq!(
        revoked.events,
        vec![
            GateEventKind::OverrideRevoked,
            GateEventKind::ProtectionLostStopped
        ]
    );

    let (recovered, _) = observe(&revoked.after, obs(H, OFF));
    assert_eq!(
        evaluate(&recovered),
        RegistrationDecision::Stopped,
        "latch created by revoke needs manual resume as well"
    );
}

#[test]
fn observation_only_changes_do_not_bump_revision_but_are_reported_as_changed() {
    let s = state(
        AdminState::Open,
        false,
        false,
        HealthEpisode::Degraded,
        obs(H, UNHEALTHY),
    );
    let outcome = apply_observation(&s, obs(H, UNKNOWN), now());
    assert!(outcome.transition.is_none());
    assert!(outcome.observation_changed);
    assert_eq!(outcome.state.revision, s.revision);
    assert_eq!(outcome.state.observation, obs(H, UNKNOWN));
}

#[test]
fn observation_only_change_refreshes_updated_at_so_returned_state_matches_storage() {
    let s = state(
        AdminState::Open,
        false,
        false,
        HealthEpisode::Degraded,
        obs(H, UNHEALTHY),
    );
    let later = OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(60);
    let changed = apply_observation(&s, obs(H, UNKNOWN), later);
    assert!(changed.transition.is_none());
    assert_eq!(changed.state.updated_at, later);

    let repeated = apply_observation(&s, obs(H, UNHEALTHY), later);
    assert_eq!(
        repeated.state.updated_at, s.updated_at,
        "no change, no write"
    );
}

#[test]
fn observation_transition_is_audited_as_a_system_action() {
    crate::ensure_generator_initialized();
    let healthy = open_state(obs(H, OFF));
    let outcome = apply_observation(&healthy, obs(OFF, OFF), now());
    let record = outcome.transition.unwrap().into_record(now());
    assert_eq!(record.audit.action, GateAction::Observation);
    assert_eq!(record.audit.actor, None);
    assert_eq!(record.expected_revision, healthy.revision);
    assert_eq!(record.events.len(), 1);
    assert_eq!(record.events[0].kind, GateEventKind::ProtectionLostStopped);
    assert_eq!(record.events[0].actor, None);
    assert_eq!(record.events[0].revision, healthy.revision + 1);
}

#[derive(Clone, Copy, Debug)]
enum Step {
    Command(GateCommand),
    Observe(RegistrationObservation),
}

fn check_invariants(s: &RegistrationGateState, path: &[Step]) {
    if s.is_override_active() {
        assert!(!s.config_lost_latch, "override excludes latch: {path:?}");
        assert_eq!(
            s.episode,
            HealthEpisode::None,
            "override ends episode: {path:?}"
        );
    }
    match s.episode {
        HealthEpisode::Outage => assert_eq!(
            classify(&s.observation),
            ProtectionHealth::Outage,
            "{path:?}"
        ),
        HealthEpisode::Degraded => assert_eq!(
            classify(&s.observation),
            ProtectionHealth::Degraded,
            "{path:?}"
        ),
        HealthEpisode::None => {}
    }
    if is_judged(s) {
        assert_ne!(
            classify(&s.observation),
            ProtectionHealth::NoConfig,
            "judged state never has nothing configured: {path:?}"
        );
    }
    let decision = evaluate(s);
    if s.admin == AdminState::Closed {
        assert_eq!(decision, RegistrationDecision::AdminClosed, "{path:?}");
    }
    if s.config_lost_latch && s.admin == AdminState::Open {
        assert_eq!(decision, RegistrationDecision::Stopped, "{path:?}");
    }
    if let RegistrationDecision::Accept { enforced, .. } = &decision {
        assert_eq!(enforced, &s.observation.effective_protections(), "{path:?}");
        if !s.is_override_active() {
            assert!(
                s.observation.meets_minimum(),
                "accept needs a ready protection: {path:?}"
            );
        }
    }
}

fn check_transition(t: &GateTransition, path: &[Step]) {
    let mut kinds: Vec<String> = t.events.iter().map(|k| format!("{k:?}")).collect();
    kinds.sort();
    kinds.dedup();
    assert_eq!(
        kinds.len(),
        t.events.len(),
        "duplicate kind in one transition: {path:?}"
    );
    assert_eq!(t.after.revision, t.before.revision + 1, "{path:?}");
}

#[test]
fn exhaustive_short_sequences_preserve_invariants() {
    let observations = [
        obs(OFF, OFF),
        obs(H, OFF),
        obs(OFF, H),
        obs(H, H),
        obs(UNHEALTHY, OFF),
        obs(UNHEALTHY, UNHEALTHY),
        obs(UNKNOWN, H),
        obs(H, UNHEALTHY),
    ];
    let mut steps: Vec<Step> = vec![
        Step::Command(GateCommand::Open),
        Step::Command(GateCommand::Close),
        Step::Command(ACK),
        Step::Command(GateCommand::RevokeOverride),
    ];
    steps.extend(observations.iter().copied().map(Step::Observe));

    let mut frontier = vec![(RegistrationGateState::initial(now()), Vec::<Step>::new())];
    let mut visited = 0usize;
    for _ in 0..4 {
        let mut next_frontier = Vec::new();
        for (s, path) in &frontier {
            for step in &steps {
                let mut path = path.clone();
                path.push(*step);
                let next = match step {
                    Step::Command(cmd) => {
                        match apply_command(s, s.revision, *cmd, &admin_id(), now()) {
                            Ok(t) => {
                                check_transition(&t, &path);
                                t.after
                            }
                            Err(_) => s.clone(),
                        }
                    }
                    Step::Observe(o) => {
                        let outcome = apply_observation(s, *o, now());
                        if let Some(t) = &outcome.transition {
                            check_transition(t, &path);
                        } else {
                            assert_eq!(outcome.state.revision, s.revision, "{path:?}");
                        }
                        outcome.state
                    }
                };
                check_invariants(&next, &path);
                visited += 1;
                next_frontier.push((next, path));
            }
        }
        frontier = next_frontier;
    }
    assert!(visited > 10_000);
}
