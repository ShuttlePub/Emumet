use super::*;

fn attestation(
    mut observation: RegistrationObservation,
    verified: bool,
) -> RegistrationObservation {
    observation.enforcement_verified = verified;
    observation
}

#[test]
fn infrastructure_loss_stops_every_open_provider_combination_even_during_override() {
    for email in protection_observations() {
        for bot in protection_observations() {
            for override_on in [false, true] {
                let before = state(
                    AdminState::Open,
                    override_on,
                    false,
                    HealthEpisode::None,
                    obs(email, bot),
                );
                let (after, events) = observe(&before, attestation(before.observation, false));
                assert_eq!(evaluate(&after), RegistrationDecision::Stopped);
                assert!(after.enforcement_lost_latch);
                assert_eq!(after.override_info, before.override_info);
                assert_eq!(events, vec![GateEventKind::EnforcementLostStopped]);
            }
        }
    }
}

#[test]
fn unverified_observation_fails_closed_even_before_a_latch_is_persisted() {
    let mut s = open_state(attestation(obs(H, H), false));
    assert_eq!(evaluate(&s), RegistrationDecision::Stopped);
    s.override_info = Some(OverrideInfo {
        activated_by: admin_id(),
        activated_at: now(),
    });
    assert_eq!(evaluate(&s), RegistrationDecision::Stopped);
    assert!(command(&s, GateCommand::Open).is_err());
    assert!(command(&s, ACK).is_err());
}

#[test]
fn restoration_notifies_once_but_never_clears_the_stop_latch() {
    let before = open_state(obs(H, H));
    let (lost, _) = observe(&before, attestation(before.observation, false));
    let (same_loss, duplicate_events) = observe(&lost, lost.observation);
    assert!(duplicate_events.is_empty());
    assert_eq!(same_loss.revision, lost.revision);

    let (restored, events) = observe(&lost, attestation(lost.observation, true));
    assert_eq!(events, vec![GateEventKind::EnforcementRestored]);
    assert_eq!(restored.revision, lost.revision + 1);
    assert!(restored.enforcement_lost_latch);
    assert_eq!(evaluate(&restored), RegistrationDecision::Stopped);
    let (_, duplicate_events) = observe(&restored, restored.observation);
    assert!(duplicate_events.is_empty());

    let resumed = run_command(&restored, GateCommand::Open).after;
    assert!(!resumed.enforcement_lost_latch);
    assert!(accepting(&resumed));
}

#[test]
fn provider_recovery_cannot_clear_infrastructure_stop() {
    let before = open_state(obs(H, H));
    let (lost, _) = observe(&before, attestation(obs(UNHEALTHY, UNHEALTHY), false));
    let (providers_recovered, events) = observe(&lost, attestation(obs(H, H), false));
    assert!(events.is_empty());
    assert!(providers_recovered.enforcement_lost_latch);
    assert_eq!(
        evaluate(&providers_recovered),
        RegistrationDecision::Stopped
    );
}

#[test]
fn unprotected_override_remains_active_but_needs_a_new_warning_ack_to_resume() {
    let before = state(
        AdminState::Open,
        true,
        false,
        HealthEpisode::None,
        obs(OFF, OFF),
    );
    let original_override = before.override_info.clone();
    let (lost, _) = observe(&before, attestation(before.observation, false));
    assert!(
        command(&lost, ACK).is_err(),
        "cannot override missing enforcement"
    );
    let (restored, _) = observe(&lost, attestation(lost.observation, true));
    assert_eq!(restored.override_info, original_override);
    assert!(command(&restored, GateCommand::Open).is_err());
    assert!(command(
        &restored,
        GateCommand::EnableOverride {
            warning_acknowledged: false
        }
    )
    .is_err());
    let resumed = run_command(&restored, ACK);
    assert!(accepting(&resumed.after));
    assert_eq!(resumed.after.override_info, original_override);
    assert!(!resumed.after.enforcement_lost_latch);
    assert!(
        !resumed.events.contains(&GateEventKind::OverrideStarted),
        "resume is not a second activation"
    );
    assert_eq!(
        run_command(&resumed.after, GateCommand::RevokeOverride)
            .after
            .override_info,
        None
    );
}

#[test]
fn repeated_true_false_edges_notify_without_auto_resuming_or_duplicate_observations() {
    let before = open_state(obs(H, OFF));
    let (lost, _) = observe(&before, attestation(before.observation, false));
    let (restored, _) = observe(&lost, attestation(lost.observation, true));
    let (lost_again, events) = observe(&restored, attestation(restored.observation, false));
    assert_eq!(events, vec![GateEventKind::EnforcementLostStopped]);
    assert_eq!(evaluate(&lost_again), RegistrationDecision::Stopped);
    let (_, same_events) = observe(&lost_again, lost_again.observation);
    assert!(same_events.is_empty());
}

#[test]
fn admin_closed_stays_closed_and_unverified_manual_open_is_refused() {
    let before = RegistrationGateState::initial(now());
    let (lost, _) = observe(&before, attestation(obs(H, H), false));
    assert_eq!(evaluate(&lost), RegistrationDecision::AdminClosed);
    assert!(command(&lost, GateCommand::Open).is_err());
    assert!(command(&lost, ACK).is_err());
    let (restored, _) = observe(&lost, attestation(lost.observation, true));
    assert_eq!(evaluate(&restored), RegistrationDecision::AdminClosed);
    assert!(accepting(&run_command(&restored, GateCommand::Open).after));
}
