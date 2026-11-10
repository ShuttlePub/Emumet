use super::*;
use crate::entity::{AuthAccountId, RegistrationObservation};

fn policy() -> WebhookDeliveryPolicy {
    WebhookDeliveryPolicy::default()
}

fn response(status: u16) -> WebhookSendObservation {
    WebhookSendObservation::Response {
        status,
        retry_after: None,
    }
}

fn decide(observation: &WebhookSendObservation, attempt: u32) -> WebhookDecision {
    decide_delivery(observation, attempt, Duration::ZERO, &policy(), 1.0)
}

#[test]
fn success_statuses_are_delivered() {
    for status in [200, 201, 202, 204, 299] {
        assert_eq!(
            decide(&response(status), 1),
            WebhookDecision::Delivered {
                http_status: Some(status)
            }
        );
    }
}

#[test]
fn transient_statuses_retry() {
    for (status, class) in [
        (408, WebhookResultClass::HttpRequestTimeout),
        (429, WebhookResultClass::HttpTooManyRequests),
        (500, WebhookResultClass::HttpServerError),
        (502, WebhookResultClass::HttpServerError),
        (503, WebhookResultClass::HttpServerError),
        (599, WebhookResultClass::HttpServerError),
    ] {
        match decide(&response(status), 1) {
            WebhookDecision::Retry {
                class: actual,
                http_status,
                delay,
            } => {
                assert_eq!(actual, class, "status {status}");
                assert_eq!(http_status, Some(status));
                assert!(delay > Duration::ZERO);
            }
            other => panic!("status {status}: expected retry, got {other:?}"),
        }
    }
}

#[test]
fn transient_transport_failures_retry() {
    for (observation, class) in [
        (WebhookSendObservation::Timeout, WebhookResultClass::Timeout),
        (
            WebhookSendObservation::ConnectFailed,
            WebhookResultClass::ConnectFailed,
        ),
        (
            WebhookSendObservation::DnsFailed,
            WebhookResultClass::DnsFailed,
        ),
    ] {
        match decide(&observation, 1) {
            WebhookDecision::Retry {
                class: actual,
                http_status,
                ..
            } => {
                assert_eq!(actual, class);
                assert_eq!(http_status, None);
            }
            other => panic!("expected retry, got {other:?}"),
        }
    }
}

#[test]
fn permanent_errors_fail_immediately_without_retry() {
    for (observation, class) in [
        (response(301), WebhookResultClass::HttpRedirect),
        (response(302), WebhookResultClass::HttpRedirect),
        (response(307), WebhookResultClass::HttpRedirect),
        (response(308), WebhookResultClass::HttpRedirect),
        (response(400), WebhookResultClass::HttpClientError),
        (response(401), WebhookResultClass::HttpClientError),
        (response(403), WebhookResultClass::HttpClientError),
        (response(404), WebhookResultClass::HttpClientError),
        (response(410), WebhookResultClass::HttpClientError),
        (response(100), WebhookResultClass::HttpUnexpectedStatus),
        (response(600), WebhookResultClass::HttpUnexpectedStatus),
        (
            WebhookSendObservation::SsrfBlocked,
            WebhookResultClass::SsrfBlocked,
        ),
        (
            WebhookSendObservation::ConfigInvalid,
            WebhookResultClass::ConfigInvalid,
        ),
    ] {
        match decide(&observation, 1) {
            WebhookDecision::Failed {
                class: actual,
                reason,
                ..
            } => {
                assert_eq!(actual, class, "{observation:?}");
                assert_eq!(reason, WebhookFailureReason::PermanentError);
            }
            other => panic!("{observation:?}: expected failed, got {other:?}"),
        }
    }
}

#[test]
fn retry_budget_exhaustion_fails_with_retries_exhausted() {
    let policy = policy();
    match decide_delivery(
        &response(503),
        policy.max_attempts,
        Duration::ZERO,
        &policy,
        0.5,
    ) {
        WebhookDecision::Failed { class, reason, .. } => {
            assert_eq!(class, WebhookResultClass::HttpServerError);
            assert_eq!(reason, WebhookFailureReason::RetriesExhausted);
        }
        other => panic!("expected failed, got {other:?}"),
    }
    assert!(matches!(
        decide_delivery(
            &response(503),
            policy.max_attempts - 1,
            Duration::ZERO,
            &policy,
            0.5
        ),
        WebhookDecision::Retry { .. }
    ));
}

#[test]
fn max_age_exhaustion_fails_even_with_attempts_left() {
    let policy = policy();
    let decision = decide_delivery(
        &WebhookSendObservation::Timeout,
        1,
        policy.max_age,
        &policy,
        0.5,
    );
    assert!(matches!(
        decision,
        WebhookDecision::Failed {
            reason: WebhookFailureReason::RetriesExhausted,
            ..
        }
    ));
}

#[test]
fn backoff_is_exponential_capped_and_jittered() {
    let policy = policy();
    assert_eq!(backoff_delay(1, &policy, 1.0), Duration::seconds(30));
    assert_eq!(backoff_delay(2, &policy, 1.0), Duration::seconds(60));
    assert_eq!(backoff_delay(3, &policy, 1.0), Duration::seconds(120));
    assert_eq!(backoff_delay(1, &policy, 0.0), Duration::seconds(15));
    assert_eq!(backoff_delay(3, &policy, 0.0), Duration::seconds(60));
    assert_eq!(backoff_delay(20, &policy, 1.0), policy.max_delay);
    assert_eq!(backoff_delay(u32::MAX, &policy, 1.0), policy.max_delay);
    assert_eq!(backoff_delay(1, &policy, 5.0), Duration::seconds(30));
    assert_eq!(backoff_delay(1, &policy, -1.0), Duration::seconds(15));
    assert_eq!(backoff_delay(1, &policy, f64::NAN), Duration::seconds(15));
    let mut previous = Duration::ZERO;
    for attempt in 1..=12 {
        let delay = backoff_delay(attempt, &policy, 0.7);
        assert!(delay >= previous);
        previous = delay;
    }
}

#[test]
fn retry_after_is_honored_with_a_cap_for_429_and_503() {
    let policy = policy();
    let with_retry_after = |status: u16, seconds: i64| WebhookSendObservation::Response {
        status,
        retry_after: Some(Duration::seconds(seconds)),
    };
    let delay_of = |observation: WebhookSendObservation| match decide_delivery(
        &observation,
        1,
        Duration::ZERO,
        &policy,
        1.0,
    ) {
        WebhookDecision::Retry { delay, .. } => delay,
        other => panic!("{other:?}"),
    };

    assert_eq!(delay_of(with_retry_after(429, 600)), Duration::seconds(600));
    assert_eq!(delay_of(with_retry_after(429, 1)), Duration::seconds(30));
    assert_eq!(
        delay_of(with_retry_after(429, 10 * 24 * 3600)),
        policy.retry_after_cap
    );
    assert_eq!(delay_of(with_retry_after(429, -5)), Duration::seconds(30));
    assert_eq!(delay_of(with_retry_after(503, 600)), Duration::seconds(600));
    assert_eq!(delay_of(with_retry_after(500, 600)), Duration::seconds(30));
}

#[test]
fn single_attempt_classification_matches_decision_classes() {
    assert_eq!(
        classify_single_attempt(&response(204)),
        (WebhookResultClass::Delivered, Some(204))
    );
    assert_eq!(
        classify_single_attempt(&response(503)),
        (WebhookResultClass::HttpServerError, Some(503))
    );
    assert_eq!(
        classify_single_attempt(&WebhookSendObservation::SsrfBlocked),
        (WebhookResultClass::SsrfBlocked, None)
    );
}

const ALL_KINDS: [GateEventKind; 6] = [
    GateEventKind::OutageStarted,
    GateEventKind::OutageRecovered,
    GateEventKind::ProtectionDegraded,
    GateEventKind::ProtectionLostStopped,
    GateEventKind::OverrideStarted,
    GateEventKind::OverrideRevoked,
];

fn sample_event(kind: GateEventKind) -> RegistrationGateEvent {
    RegistrationGateEvent {
        id: RegistrationGateEventId::new(123456789),
        kind,
        revision: 7,
        actor: Some(AuthAccountId::new(42)),
        observation: RegistrationObservation::new(
            ProtectionObservation::new(true, ProbeStatus::Unhealthy),
            ProtectionObservation::new(false, ProbeStatus::Unknown),
        ),
        created_at: OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap(),
    }
}

fn sample_snapshot() -> GateControlSnapshot {
    GateControlSnapshot {
        admin: AdminState::Open,
        override_active: false,
        config_lost_latch: false,
        episode: HealthEpisode::Outage,
        revision: 7,
    }
}

#[test]
fn payload_contains_exactly_the_allowlisted_fields() {
    let event = sample_event(GateEventKind::OutageStarted);
    let payload = WebhookPayload::from_gate_event(&event, &sample_snapshot());
    let value: serde_json::Value = serde_json::from_str(&payload.to_json().unwrap()).unwrap();
    assert_eq!(
        value,
        serde_json::json!({
            "version": 1,
            "event_id": "123456789",
            "type": "outage_started",
            "occurred_at": "2023-11-14T22:13:20Z",
            "gate": {
                "admin_state": "open",
                "override_active": false,
                "config_lost_latch": false,
                "episode": "outage",
                "revision": 7
            },
            "email_verification": {"configured": true, "probe": "unhealthy"},
            "bot_protection": {"configured": false, "probe": "unknown"}
        })
    );
}

#[test]
fn payload_never_carries_actor_email_secret_or_error_detail() {
    for kind in ALL_KINDS {
        let event = sample_event(kind);
        let json = WebhookPayload::from_gate_event(&event, &sample_snapshot())
            .to_json()
            .unwrap();
        for forbidden in [
            "actor", "\"42\"", "@", "secret", "password", "token", "error", "url",
        ] {
            assert!(
                !json.contains(forbidden),
                "payload for {kind:?} must not contain `{forbidden}`: {json}"
            );
        }
    }
}

#[test]
fn every_gate_event_kind_has_a_webhook_event_type_with_the_same_name() {
    for kind in ALL_KINDS {
        let gate_name = serde_json::to_value(kind).unwrap();
        let webhook = WebhookEventType::from(kind);
        assert_eq!(serde_json::to_value(webhook).unwrap(), gate_name);
        assert_eq!(gate_name.as_str().unwrap(), webhook.as_str());
    }
    assert_eq!(
        serde_json::to_value(WebhookEventType::WebhookTest).unwrap(),
        "webhook_test"
    );
}

#[test]
fn event_ids_are_stable_and_test_ids_do_not_collide_with_gate_ids() {
    let event = sample_event(GateEventKind::OverrideStarted);
    let first = WebhookPayload::from_gate_event(&event, &sample_snapshot());
    let second = WebhookPayload::from_gate_event(&event, &sample_snapshot());
    assert_eq!(first.event_id, "123456789");
    assert_eq!(first.to_json().unwrap(), second.to_json().unwrap());
    assert_eq!(test_event_wire_id(99), "test_99");
    assert!(test_event_wire_id(123456789).parse::<i64>().is_err());
}

#[test]
fn test_payload_has_no_gate_state() {
    let payload = WebhookPayload::test(5, OffsetDateTime::from_unix_timestamp(0).unwrap());
    let value: serde_json::Value = serde_json::from_str(&payload.to_json().unwrap()).unwrap();
    assert_eq!(value["type"], "webhook_test");
    assert_eq!(value["event_id"], "test_5");
    assert!(value.get("gate").is_none());
}

#[test]
fn new_delivery_is_built_from_gate_events_only() {
    let event = sample_event(GateEventKind::ProtectionDegraded);
    let delivery = NewWebhookDelivery::from_gate_event(&event, &sample_snapshot()).unwrap();
    assert_eq!(delivery.event_id, event.id);
    assert_eq!(delivery.kind, GateEventKind::ProtectionDegraded);
    assert!(delivery.payload_json.contains("\"protection_degraded\""));
}

#[test]
fn config_status_serializes_without_secrets() {
    let enabled = WebhookConfigStatus::Enabled {
        host: "hooks.example.com".to_string(),
    };
    assert_eq!(
        serde_json::to_value(&enabled).unwrap(),
        serde_json::json!({"state": "enabled", "host": "hooks.example.com"})
    );
    assert_eq!(
        serde_json::to_value(WebhookConfigStatus::Disabled).unwrap(),
        serde_json::json!({"state": "disabled"})
    );
    assert_eq!(
        serde_json::to_value(WebhookConfigStatus::Misconfigured {
            issue: WebhookConfigIssue::UnsupportedScheme
        })
        .unwrap(),
        serde_json::json!({"state": "misconfigured", "issue": "unsupported_scheme"})
    );
}
