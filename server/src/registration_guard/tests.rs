use super::*;
use axum::{body::Body, http::Request};
use driver::registration_protection::EnvSource;
use http_body_util::BodyExt;
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};
use tower::ServiceExt;

const SECRET: &str = "test-only-guard-secret-at-least-32-bytes";

#[derive(Clone)]
struct Fake {
    verdict: RegistrationVerdict,
    delay: Duration,
    calls: Arc<AtomicUsize>,
}
impl GuardService for Fake {
    async fn check(&self, _: HookContext) -> RegistrationVerdict {
        self.calls.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(self.delay).await;
        self.verdict
    }
}

fn config() -> GuardConfig {
    GuardConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        allowed_peers: vec!["127.0.0.1".parse().unwrap()],
        secret: SECRET.into(),
        deadline: Duration::from_millis(50),
        period: Duration::from_secs(30),
    }
}

fn valid_body() -> serde_json::Value {
    serde_json::json!({
        "flow_id": "f68e7ed2-b053-430d-acba-22883c94c998",
        "flow_type": "api",
        "email": "test@example.com",
        "email_proof": "secret-proof",
        "bot_token": "secret-token"
    })
}

fn request(secret: Option<&str>, peer: &str, body: String) -> Request<Body> {
    let mut request = Request::builder()
        .method("POST")
        .uri("/registration/pre-persist")
        .extension(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
    if let Some(secret) = secret {
        request = request.header("x-emumet-registration-key", secret);
    }
    request.body(Body::from(body)).unwrap()
}

fn fake(verdict: RegistrationVerdict) -> Fake {
    Fake {
        verdict,
        delay: Duration::ZERO,
        calls: Arc::new(AtomicUsize::new(0)),
    }
}

#[tokio::test]
async fn hook_returns_only_empty_204_or_structured_403_without_sensitive_context() {
    for verdict in [
        RegistrationVerdict::Accept,
        RegistrationVerdict::Reject,
        RegistrationVerdict::TemporarilyUnavailable,
    ] {
        let service = fake(verdict);
        let response = router(config(), service.clone())
            .oneshot(request(
                Some(SECRET),
                "127.0.0.1:123",
                valid_body().to_string(),
            ))
            .await
            .unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(service.calls.load(Ordering::SeqCst), 1);
        if verdict == RegistrationVerdict::Accept {
            assert_eq!(status, StatusCode::NO_CONTENT);
            assert!(body.is_empty());
        } else {
            assert_eq!(status, StatusCode::FORBIDDEN);
            let messages: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(messages["messages"][0]["instance_ptr"], "#/");
            let text = String::from_utf8_lossy(&body);
            for sensitive in [SECRET, "secret-proof", "secret-token", "test@example.com"] {
                assert!(!text.contains(sensitive));
            }
        }
    }
}

#[tokio::test]
async fn hook_authentication_peer_allowlist_and_strict_context_fail_closed() {
    let service = fake(RegistrationVerdict::Accept);
    let app = router(config(), service.clone());
    let mut unknown = valid_body();
    unknown["override"] = true.into();
    let mut credentials = valid_body();
    credentials["credentials"] = serde_json::json!({"password": "must-not-be-forwarded"});
    let mut flow = valid_body();
    flow["flow_id"] = "invalid-flow".into();
    let mut kind = valid_body();
    kind["flow_type"] = "login".into();
    for (secret, peer, body) in [
        (None, "127.0.0.1:1", valid_body().to_string()),
        (Some("incorrect"), "127.0.0.1:1", valid_body().to_string()),
        (Some(SECRET), "192.0.2.10:1", valid_body().to_string()),
        (Some(SECRET), "127.0.0.1:1", unknown.to_string()),
        (Some(SECRET), "127.0.0.1:1", credentials.to_string()),
        (Some(SECRET), "127.0.0.1:1", flow.to_string()),
        (Some(SECRET), "127.0.0.1:1", kind.to_string()),
        (Some(SECRET), "127.0.0.1:1", "{".into()),
        (Some(SECRET), "127.0.0.1:1", "x".repeat(16385)),
    ] {
        assert_eq!(
            app.clone()
                .oneshot(request(secret, peer, body))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    let mut get = request(Some(SECRET), "127.0.0.1:1", valid_body().to_string());
    *get.method_mut() = Method::GET;
    assert_eq!(
        app.oneshot(get).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(service.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn hook_deadline_is_a_non_retryable_403_not_a_5xx() {
    let mut service = fake(RegistrationVerdict::Accept);
    service.delay = Duration::from_secs(1);
    let started = tokio::time::Instant::now();
    let response = router(config(), service)
        .oneshot(request(
            Some(SECRET),
            "127.0.0.1:1",
            valid_body().to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(started.elapsed() < Duration::from_millis(500));
}

#[test]
fn hook_config_is_opt_in_and_never_prints_secret_or_bad_values() {
    assert!(GuardConfig::from_source(&EnvSource::new())
        .unwrap()
        .is_none());
    let source = EnvSource::new()
        .with("REGISTRATION_ENFORCEMENT_ENABLED", "true")
        .with("REGISTRATION_HOOK_SECRET", SECRET);
    let config = GuardConfig::from_source(&source).unwrap().unwrap();
    assert!(config.listen.ip().is_loopback());
    assert_eq!(config.deadline, Duration::from_secs(5));
    for source in [
        source.clone().with("REGISTRATION_HOOK_SECRET", "short"),
        source
            .clone()
            .with("REGISTRATION_HOOK_SECRET_FILE", "/not-present"),
        source
            .clone()
            .with("REGISTRATION_HOOK_ALLOWED_PEERS", "0.0.0.0/0"),
        source
            .clone()
            .with("REGISTRATION_PROTECTION_POLL_SECS", "0"),
    ] {
        let error = GuardConfig::from_source(&source).err().unwrap();
        assert!(!error.contains(SECRET));
    }
}
