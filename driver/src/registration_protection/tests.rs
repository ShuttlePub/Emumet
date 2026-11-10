//! 登録保護 adapter の contract test: 設定読込・設定世代・Turnstile Siteverify の分類 (E15b の実 API 応答を
//! fixture 化)・実 HTTP 経路 (wiremock) ・SMTP の実送信経路 (ローカル SMTP 模擬)。
//! 外部ネットワークを使うのは `#[ignore]` の live テストだけ (README / docs 参照)。

mod smtp_mock;

use super::config::{secret_fingerprint, ProcessEnv, ValueSource};
use super::smtp::{classify_smtp_failure, SmtpFailure, SmtpStage};
use super::*;
use kernel::interfaces::repository::{BotTokenCheck, EmailTransport, TurnstileVerifier};
use kernel::prelude::entity::{ProviderOutcome, ProviderResult, ReasonCode, TransportClass};
use smtp_mock::{MockSmtp, SmtpScript};
use std::time::Duration;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const HOSTNAME: &str = "example.com";
const SECRET: &str = "turnstile-secret-0123456789abcdef-not-real";

fn expectation() -> SiteverifyExpectation {
    SiteverifyExpectation {
        hostname: HOSTNAME.to_string(),
        action: None,
        token_max_age: time::Duration::seconds(300),
    }
}

fn now_ts() -> String {
    OffsetDateTime::now_utc().format(&Rfc3339).unwrap()
}

fn success_body(hostname: &str, action: Option<&str>, ts: &str) -> String {
    let mut value = serde_json::json!({
        "success": true,
        "error-codes": [],
        "hostname": hostname,
        "challenge_ts": ts,
    });
    if let Some(action) = action {
        value["action"] = serde_json::Value::String(action.to_string());
    }
    value.to_string()
}

fn failure_body(codes: &[&str]) -> String {
    serde_json::json!({"success": false, "error-codes": codes}).to_string()
}

// ---- 設定・generation -----------------------------------------------------------------------

fn turnstile_env() -> EnvSource {
    EnvSource::new()
        .with("REGISTRATION_TURNSTILE_SITE_KEY", "0x4AAAA-site")
        .with("REGISTRATION_TURNSTILE_SECRET", SECRET)
        .with("REGISTRATION_TURNSTILE_EXPECTED_HOSTNAME", HOSTNAME)
}

#[test]
fn unset_empty_and_disabled_turnstile_configuration_is_configuration_invalid() {
    let unset = TurnstileSettings::from_source(&EnvSource::new());
    assert!(!unset.snapshot().enabled);
    assert_eq!(unset.snapshot().issue, Some(ReasonCode::NotConfigured));

    // 空文字・空白だけは未設定と同じ。
    let empty = TurnstileSettings::from_source(
        &EnvSource::new()
            .with("REGISTRATION_TURNSTILE_SECRET", "   ")
            .with("REGISTRATION_TURNSTILE_SITE_KEY", ""),
    );
    assert_eq!(empty.snapshot().issue, Some(ReasonCode::NotConfigured));

    let disabled = TurnstileSettings::from_source(
        &turnstile_env().with("REGISTRATION_TURNSTILE_ENABLED", "false"),
    );
    assert!(!disabled.snapshot().enabled);
    assert_eq!(disabled.snapshot().issue, Some(ReasonCode::Disabled));

    let ok = TurnstileSettings::from_source(&turnstile_env());
    assert!(ok.snapshot().is_usable());
}

#[test]
fn incomplete_or_ambiguous_turnstile_configuration_is_configuration_invalid() {
    let missing_secret = TurnstileSettings::from_source(
        &EnvSource::new().with("REGISTRATION_TURNSTILE_EXPECTED_HOSTNAME", HOSTNAME),
    );
    assert_eq!(
        missing_secret.snapshot().issue,
        Some(ReasonCode::MissingSecret)
    );

    let missing_hostname = TurnstileSettings::from_source(
        &EnvSource::new().with("REGISTRATION_TURNSTILE_SECRET", SECRET),
    );
    assert_eq!(
        missing_hostname.snapshot().issue,
        Some(ReasonCode::InvalidConfiguration)
    );

    let bad_age = TurnstileSettings::from_source(
        &turnstile_env().with("REGISTRATION_TURNSTILE_TOKEN_MAX_AGE_SECS", "soon"),
    );
    assert_eq!(
        bad_age.snapshot().issue,
        Some(ReasonCode::InvalidConfiguration)
    );

    let both = TurnstileSettings::from_source(
        &turnstile_env().with("REGISTRATION_TURNSTILE_SECRET_FILE", "/nonexistent"),
    );
    assert_eq!(
        both.snapshot().issue,
        Some(ReasonCode::InvalidConfiguration)
    );

    let unreadable = TurnstileSettings::from_source(
        &EnvSource::new()
            .with("REGISTRATION_TURNSTILE_SECRET_FILE", "/nonexistent/secret")
            .with("REGISTRATION_TURNSTILE_EXPECTED_HOSTNAME", HOSTNAME),
    );
    assert_eq!(unreadable.snapshot().issue, Some(ReasonCode::MissingSecret));
}

#[test]
fn turnstile_secret_can_come_from_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("secret");
    std::fs::write(&file, format!("{SECRET}\n")).unwrap();
    let from_file = TurnstileSettings::from_source(
        &EnvSource::new()
            .with("REGISTRATION_TURNSTILE_SECRET_FILE", file.to_str().unwrap())
            .with("REGISTRATION_TURNSTILE_EXPECTED_HOSTNAME", HOSTNAME)
            .with("REGISTRATION_TURNSTILE_SITE_KEY", "0x4AAAA-site"),
    );
    let direct = TurnstileSettings::from_source(&turnstile_env());
    assert!(from_file.snapshot().is_usable());
    // 値が同じなら取得元によらず同じ世代。
    assert_eq!(
        from_file.snapshot().generation,
        direct.snapshot().generation
    );
}

#[test]
fn generation_is_deterministic_and_changes_with_every_effective_setting() {
    let base = TurnstileSettings::from_source(&turnstile_env());
    let same = TurnstileSettings::from_source(&turnstile_env());
    assert_eq!(base.snapshot().generation, same.snapshot().generation);

    let variants = [
        turnstile_env().with(
            "REGISTRATION_TURNSTILE_SECRET",
            "another-secret-value-0123456789",
        ),
        turnstile_env().with("REGISTRATION_TURNSTILE_SITE_KEY", "0x4AAAA-other"),
        turnstile_env().with("REGISTRATION_TURNSTILE_EXPECTED_HOSTNAME", "other.example"),
        turnstile_env().with("REGISTRATION_TURNSTILE_EXPECTED_ACTION", "register"),
        turnstile_env().with("REGISTRATION_TURNSTILE_TOKEN_MAX_AGE_SECS", "120"),
        turnstile_env().with("REGISTRATION_TURNSTILE_ENABLED", "false"),
    ];
    let mut seen = vec![base.snapshot().generation.clone()];
    for variant in variants {
        let generation = TurnstileSettings::from_source(&variant)
            .snapshot()
            .generation
            .clone();
        assert!(
            !seen.contains(&generation),
            "{generation:?} collides with a previous generation"
        );
        seen.push(generation);
    }
}

#[test]
fn secrets_never_appear_in_snapshots_debug_output_or_the_generation() {
    let settings = TurnstileSettings::from_source(&turnstile_env());
    let snapshot = settings.snapshot();
    let rendered = format!("{settings:?} {snapshot:?}");
    assert!(!rendered.contains(SECRET));
    assert!(!snapshot.generation.as_str().contains(SECRET));
    assert!(!snapshot.fingerprint.contains(SECRET));
    assert_eq!(snapshot.fingerprint, secret_fingerprint(SECRET.as_bytes()));
    assert_eq!(snapshot.fingerprint.len(), 12);
    let verifier = HttpTurnstileVerifier::new(settings);
    assert!(!format!("{verifier:?}").contains(SECRET));
}

fn smtp_env(port: u16) -> EnvSource {
    EnvSource::new()
        .with("REGISTRATION_SMTP_HOST", "127.0.0.1")
        .with("REGISTRATION_SMTP_PORT", &port.to_string())
        .with("REGISTRATION_SMTP_TLS", "plaintext")
        .with("REGISTRATION_SMTP_USERNAME", "mailer")
        .with("REGISTRATION_SMTP_PASSWORD", "smtp-password-0123456789")
        .with("REGISTRATION_EMAIL_FROM", "Emumet <no-reply@example.com>")
        .with("REGISTRATION_EMAIL_CANARY_ADDRESS", "canary@example.com")
}

#[test]
fn smtp_configuration_is_classified_and_plaintext_is_not_available_in_production_parsing() {
    let unset = SmtpSettings::from_source(&EnvSource::new());
    assert_eq!(unset.snapshot().issue, Some(ReasonCode::NotConfigured));
    assert!(!unset.snapshot().enabled);

    let disabled =
        SmtpSettings::from_source(&smtp_env(2525).with("REGISTRATION_EMAIL_ENABLED", "false"));
    assert_eq!(disabled.snapshot().issue, Some(ReasonCode::Disabled));

    // 本番の読込 (from_source) では平文 SMTP を選べない。
    let production = SmtpSettings::from_source(&smtp_env(2525));
    assert_eq!(
        production.snapshot().issue,
        Some(ReasonCode::InvalidConfiguration)
    );

    let test_mode = SmtpSettings::from_source_allowing_plaintext(&smtp_env(2525));
    assert!(test_mode.snapshot().is_usable());

    let ok = SmtpSettings::from_source(
        &EnvSource::new()
            .with("REGISTRATION_SMTP_HOST", "smtp.example.com")
            .with("REGISTRATION_EMAIL_FROM", "no-reply@example.com"),
    );
    assert!(
        ok.snapshot().is_usable(),
        "starttls is the default: {:?}",
        ok.snapshot().issue
    );
    let bad_host = SmtpSettings::from_source(
        &EnvSource::new()
            .with("REGISTRATION_SMTP_HOST", "bad host!")
            .with("REGISTRATION_EMAIL_FROM", "no-reply@example.com"),
    );
    assert_eq!(
        bad_host.snapshot().issue,
        Some(ReasonCode::InvalidConfiguration)
    );
    let no_host = SmtpSettings::from_source(
        &EnvSource::new().with("REGISTRATION_EMAIL_FROM", "no-reply@example.com"),
    );
    assert_eq!(
        no_host.snapshot().issue,
        Some(ReasonCode::InvalidConfiguration)
    );
    let bad_from = SmtpSettings::from_source(
        &EnvSource::new()
            .with("REGISTRATION_SMTP_HOST", "smtp.example.com")
            .with("REGISTRATION_EMAIL_FROM", "not an address"),
    );
    assert_eq!(
        bad_from.snapshot().issue,
        Some(ReasonCode::InvalidConfiguration)
    );
    let bad_port = SmtpSettings::from_source(
        &EnvSource::new()
            .with("REGISTRATION_SMTP_HOST", "smtp.example.com")
            .with("REGISTRATION_SMTP_PORT", "0")
            .with("REGISTRATION_EMAIL_FROM", "no-reply@example.com"),
    );
    assert_eq!(
        bad_port.snapshot().issue,
        Some(ReasonCode::InvalidConfiguration)
    );
    let user_without_password = SmtpSettings::from_source(
        &EnvSource::new()
            .with("REGISTRATION_SMTP_HOST", "smtp.example.com")
            .with("REGISTRATION_SMTP_USERNAME", "mailer")
            .with("REGISTRATION_EMAIL_FROM", "no-reply@example.com"),
    );
    assert_eq!(
        user_without_password.snapshot().issue,
        Some(ReasonCode::MissingSecret)
    );
    let bad_canary = SmtpSettings::from_source(
        &EnvSource::new()
            .with("REGISTRATION_SMTP_HOST", "smtp.example.com")
            .with("REGISTRATION_EMAIL_FROM", "no-reply@example.com")
            .with("REGISTRATION_EMAIL_CANARY_ADDRESS", "not-an-address"),
    );
    assert_eq!(
        bad_canary.snapshot().issue,
        Some(ReasonCode::InvalidConfiguration)
    );
}

#[test]
fn smtp_generation_changes_with_host_port_credentials_from_and_canary() {
    let base = SmtpSettings::from_source_allowing_plaintext(&smtp_env(2525));
    let same = SmtpSettings::from_source_allowing_plaintext(&smtp_env(2525));
    assert_eq!(base.snapshot().generation, same.snapshot().generation);
    let mut seen = vec![base.snapshot().generation.clone()];
    for variant in [
        smtp_env(2526),
        smtp_env(2525).with("REGISTRATION_SMTP_HOST", "localhost"),
        smtp_env(2525).with("REGISTRATION_SMTP_USERNAME", "other"),
        smtp_env(2525).with("REGISTRATION_SMTP_PASSWORD", "another-password-0123456789"),
        smtp_env(2525).with("REGISTRATION_EMAIL_FROM", "other@example.com"),
        smtp_env(2525).with("REGISTRATION_EMAIL_CANARY_ADDRESS", "canary2@example.com"),
    ] {
        let generation = SmtpSettings::from_source_allowing_plaintext(&variant)
            .snapshot()
            .generation
            .clone();
        assert!(!seen.contains(&generation));
        seen.push(generation);
    }
    let rendered = format!("{base:?} {:?}", base.snapshot());
    assert!(!rendered.contains("smtp-password"));
}

#[test]
fn process_env_source_reads_the_environment() {
    // 未設定の環境変数は None (この名前は他のどこでも設定されない)。
    assert_eq!(ProcessEnv.get("REGISTRATION_E15C_UNSET_FOR_TEST"), None);
}

// ---- Turnstile: E15b の実 API 応答を fixture 化した分類 ---------------------------------------

fn provider_fixture() -> serde_json::Value {
    serde_json::from_str(include_str!(
        "../../../e2e/registration-guard/evidence/provider.json"
    ))
    .unwrap()
}

fn fixture_case(name: &str) -> (u16, Vec<u8>) {
    let fixture = provider_fixture();
    let case = &fixture["results"][name];
    (
        case["http_status"].as_u64().unwrap() as u16,
        serde_json::to_vec(&case["body"]).unwrap(),
    )
}

#[test]
fn real_siteverify_responses_from_e15b_are_classified() {
    let fixture = provider_fixture();
    let challenge_ts = fixture["results"]["test_pass"]["body"]["challenge_ts"]
        .as_str()
        .unwrap();
    let issued = OffsetDateTime::parse(challenge_ts, &Rfc3339).unwrap();

    let (status, body) = fixture_case("test_pass");
    assert_eq!(
        classify_siteverify(
            status,
            &body,
            &expectation(),
            issued + time::Duration::seconds(1)
        ),
        ProviderResult::business_success(TransportClass::Http2xx)
    );
    // HTTP 200 + success:false は失敗 (invalid / duplicate / missing response は当該 token だけの拒否)。
    for (name, reason) in [
        ("test_reject", ReasonCode::TokenRejected),
        ("test_duplicate", ReasonCode::TokenDuplicate),
        ("empty_response", ReasonCode::TokenRejected),
    ] {
        let (status, body) = fixture_case(name);
        assert_eq!(status, 200, "{name}");
        assert_eq!(
            classify_siteverify(status, &body, &expectation(), issued),
            ProviderResult::attempt_rejected(reason, TransportClass::Http2xx),
            "{name}"
        );
    }
    // secret の不備 (HTTP 400) は configuration_invalid。
    for (name, reason) in [
        ("empty_secret", ReasonCode::MissingSecret),
        ("invalid_secret", ReasonCode::InvalidSecret),
    ] {
        let (status, body) = fixture_case(name);
        assert_eq!(status, 400, "{name}");
        assert_eq!(
            classify_siteverify(status, &body, &expectation(), issued),
            ProviderResult::configuration_invalid(reason, TransportClass::Http4xx),
            "{name}"
        );
    }
}

#[test]
fn success_requires_hostname_action_and_fresh_token() {
    let now = OffsetDateTime::now_utc();
    let classify = |expected: &SiteverifyExpectation, body: &str| {
        classify_siteverify(200, body.as_bytes(), expected, now)
    };
    let ok = success_body(HOSTNAME, Some("register"), &now_ts());
    let with_action = SiteverifyExpectation {
        action: Some("register".to_string()),
        ..expectation()
    };
    assert_eq!(
        classify(&expectation(), &ok).outcome,
        ProviderOutcome::BusinessSuccess
    );
    assert_eq!(
        classify(&with_action, &ok).outcome,
        ProviderOutcome::BusinessSuccess
    );
    // hostname の大文字小文字は区別しない。
    assert_eq!(
        classify(
            &expectation(),
            &success_body("EXAMPLE.com", None, &now_ts())
        )
        .outcome,
        ProviderOutcome::BusinessSuccess
    );

    assert_eq!(
        classify(
            &expectation(),
            &success_body("evil.example", None, &now_ts())
        ),
        ProviderResult::attempt_rejected(ReasonCode::HostnameMismatch, TransportClass::Http2xx)
    );
    assert_eq!(
        classify(
            &expectation(),
            r#"{"success":true,"challenge_ts":"2999-01-01T00:00:00Z"}"#
        )
        .reason,
        ReasonCode::HostnameMismatch,
        "a success without a hostname is not accepted"
    );
    assert_eq!(
        classify(
            &with_action,
            &success_body(HOSTNAME, Some("login"), &now_ts())
        ),
        ProviderResult::attempt_rejected(ReasonCode::ActionMismatch, TransportClass::Http2xx)
    );
    assert_eq!(
        classify(&with_action, &success_body(HOSTNAME, None, &now_ts())).reason,
        ReasonCode::ActionMismatch
    );
    // token の期限: 300 秒以内は有効、超えたら拒否。未来の時刻・欠落・不正な時刻も拒否。
    let old = (now - time::Duration::seconds(301))
        .format(&Rfc3339)
        .unwrap();
    let edge = (now - time::Duration::seconds(299))
        .format(&Rfc3339)
        .unwrap();
    let future = (now + time::Duration::minutes(10))
        .format(&Rfc3339)
        .unwrap();
    assert_eq!(
        classify(&expectation(), &success_body(HOSTNAME, None, &old)).reason,
        ReasonCode::TokenExpired
    );
    assert_eq!(
        classify(&expectation(), &success_body(HOSTNAME, None, &edge)).outcome,
        ProviderOutcome::BusinessSuccess
    );
    assert_eq!(
        classify(&expectation(), &success_body(HOSTNAME, None, &future)).reason,
        ReasonCode::TokenExpired
    );
    assert_eq!(
        classify(
            &expectation(),
            &format!(r#"{{"success":true,"hostname":"{HOSTNAME}"}}"#)
        )
        .reason,
        ReasonCode::TokenExpired
    );
    assert_eq!(
        classify(&expectation(), &success_body(HOSTNAME, None, "yesterday")).reason,
        ReasonCode::TokenExpired
    );
}

#[test]
fn http_and_protocol_failures_are_temporarily_unavailable() {
    let now = OffsetDateTime::now_utc();
    let classify =
        |status: u16, body: &str| classify_siteverify(status, body.as_bytes(), &expectation(), now);
    assert_eq!(
        classify(429, "{}"),
        ProviderResult::temporarily_unavailable(
            ReasonCode::ProviderRateLimited,
            TransportClass::Http429
        )
    );
    for status in [500, 502, 503, 504] {
        assert_eq!(
            classify(status, ""),
            ProviderResult::temporarily_unavailable(
                ReasonCode::ProviderServerError,
                TransportClass::Http5xx
            ),
            "{status}"
        );
    }
    // 不正な応答 (JSON でない・success 欠落・未知の error code・3xx・success:true なのに非 2xx)。
    for (status, body) in [
        (200, "<html>oops</html>"),
        (200, ""),
        (200, "{}"),
        (200, r#"{"error-codes":["invalid-input-response"]}"#),
        (
            200,
            r#"{"success":false,"error-codes":["brand-new-error"]}"#,
        ),
        (200, r#"{"success":false}"#),
        (302, ""),
        (400, "not json"),
        (403, ""),
        (200, r#"{"success":"yes"}"#),
        (500 - 100, &success_body(HOSTNAME, None, &now_ts())),
    ] {
        let result = classify(status, body);
        assert_eq!(
            result.outcome,
            ProviderOutcome::TemporarilyUnavailable,
            "{status} {body}"
        );
        assert_eq!(
            result.reason,
            ReasonCode::MalformedResponse,
            "{status} {body}"
        );
    }
    // 利用者の不正 token (invalid / duplicate / missing) は provider 全体を障害扱いしない。
    for code in [
        "invalid-input-response",
        "missing-input-response",
        "timeout-or-duplicate",
        "bad-request",
    ] {
        assert_eq!(
            classify(200, &failure_body(&[code])).outcome,
            ProviderOutcome::AttemptRejected,
            "{code}"
        );
    }
    // secret 起因が token 起因と同居しても secret が優先される。
    assert_eq!(
        classify(
            400,
            &failure_body(&["invalid-input-response", "invalid-input-secret"])
        )
        .outcome,
        ProviderOutcome::ConfigurationInvalid
    );
    assert_eq!(
        classify(200, &failure_body(&["internal-error"])).reason,
        ReasonCode::ProviderServerError
    );
}

// ---- Turnstile: 実 HTTP 経路 (fake provider = wiremock) --------------------------------------

fn verifier(server: &MockServer) -> HttpTurnstileVerifier {
    HttpTurnstileVerifier::new(TurnstileSettings::from_source(&turnstile_env()))
        .with_test_base_url(&format!("{}/turnstile/v0/siteverify", server.uri()))
        .with_network_settings(TurnstileNetworkSettings {
            connect_timeout: Duration::from_millis(300),
            total_timeout: Duration::from_millis(600),
            max_response_bytes: 1024,
        })
}

fn check(token: &str) -> BotTokenCheck<'_> {
    BotTokenCheck {
        token,
        remote_ip: None,
    }
}

#[tokio::test]
async fn production_verifier_targets_the_official_https_endpoint_only() {
    let verifier = HttpTurnstileVerifier::new(TurnstileSettings::from_source(&turnstile_env()));
    assert_eq!(
        verifier.base_url_for_test(),
        "https://challenges.cloudflare.com/turnstile/v0/siteverify"
    );
    assert!(!verifier.allows_insecure_for_test());
}

#[tokio::test]
async fn verify_sends_the_secret_and_token_and_succeeds_only_on_a_business_success() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/turnstile/v0/siteverify"))
        .and(body_string_contains("secret=turnstile-secret"))
        .and(body_string_contains("response=user-token-1"))
        .and(body_string_contains("remoteip=203.0.113.9"))
        .respond_with(ResponseTemplate::new(200).set_body_string(success_body(
            HOSTNAME,
            None,
            &now_ts(),
        )))
        .expect(1)
        .mount(&server)
        .await;
    let result = verifier(&server)
        .verify(&BotTokenCheck {
            token: "user-token-1",
            remote_ip: Some("203.0.113.9"),
        })
        .await;
    assert_eq!(
        result,
        ProviderResult::business_success(TransportClass::Http2xx)
    );
}

#[tokio::test]
async fn http_200_with_success_false_is_a_failure_and_an_invalid_token_does_not_down_the_provider()
{
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(failure_body(&["invalid-input-response"])),
        )
        .mount(&server)
        .await;
    let result = verifier(&server).verify(&check("bad-token")).await;
    assert_eq!(
        result,
        ProviderResult::attempt_rejected(ReasonCode::TokenRejected, TransportClass::Http2xx)
    );
}

#[tokio::test]
async fn empty_or_oversized_tokens_are_rejected_without_calling_the_provider() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string(success_body(
            HOSTNAME,
            None,
            &now_ts(),
        )))
        .expect(0)
        .mount(&server)
        .await;
    let verifier = verifier(&server);
    for token in ["", "   ", &"x".repeat(4096)] {
        let result = verifier.verify(&check(token)).await;
        assert_eq!(result.outcome, ProviderOutcome::AttemptRejected);
        assert_eq!(result.transport, TransportClass::None);
    }
}

#[tokio::test]
async fn provider_http_failures_are_classified_over_the_wire() {
    for (status, reason, transport) in [
        (
            429,
            ReasonCode::ProviderRateLimited,
            TransportClass::Http429,
        ),
        (
            500,
            ReasonCode::ProviderServerError,
            TransportClass::Http5xx,
        ),
        (
            503,
            ReasonCode::ProviderServerError,
            TransportClass::Http5xx,
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(status).set_body_string("slow down"))
            .mount(&server)
            .await;
        assert_eq!(
            verifier(&server).verify(&check("token")).await,
            ProviderResult::temporarily_unavailable(reason, transport),
            "{status}"
        );
    }
    // 無効な secret (HTTP 400 + invalid-input-secret) は configuration_invalid。
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(400).set_body_string(failure_body(&["invalid-input-secret"])),
        )
        .mount(&server)
        .await;
    assert_eq!(
        verifier(&server).verify(&check("token")).await,
        ProviderResult::configuration_invalid(ReasonCode::InvalidSecret, TransportClass::Http4xx)
    );
}

#[tokio::test]
async fn slow_unreachable_oversized_and_redirected_responses_are_temporarily_unavailable() {
    // timeout (応答が total_timeout より遅い)。
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(success_body(HOSTNAME, None, &now_ts()))
                .set_delay(Duration::from_secs(3)),
        )
        .mount(&server)
        .await;
    let started = std::time::Instant::now();
    assert_eq!(
        verifier(&server).verify(&check("token")).await,
        ProviderResult::temporarily_unavailable(ReasonCode::Timeout, TransportClass::Timeout)
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the provider deadline is short"
    );

    // 接続不能 (閉じたポート)。
    let closed = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let unreachable = HttpTurnstileVerifier::new(TurnstileSettings::from_source(&turnstile_env()))
        .with_test_base_url(&format!("http://127.0.0.1:{closed}/siteverify"));
    assert_eq!(
        unreachable.verify(&check("token")).await,
        ProviderResult::temporarily_unavailable(
            ReasonCode::ConnectFailed,
            TransportClass::ConnectFailed
        )
    );

    // 応答本文のサイズ上限。
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string("x".repeat(4096)))
        .mount(&server)
        .await;
    assert_eq!(
        verifier(&server).verify(&check("token")).await,
        ProviderResult::temporarily_unavailable(
            ReasonCode::MalformedResponse,
            TransportClass::MalformedBody
        )
    );

    // redirect は追従しない (3xx は不正応答として一時障害)。
    let server = MockServer::start().await;
    let target = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("location", format!("{}/elsewhere", target.uri())),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string(success_body(
            HOSTNAME,
            None,
            &now_ts(),
        )))
        .expect(0)
        .mount(&target)
        .await;
    let result = verifier(&server).verify(&check("token")).await;
    assert_eq!(result.outcome, ProviderOutcome::TemporarilyUnavailable);
    assert_eq!(result.transport, TransportClass::HttpOther);
}

#[tokio::test]
async fn unusable_configuration_never_calls_the_provider() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let unconfigured =
        HttpTurnstileVerifier::new(TurnstileSettings::from_source(&EnvSource::new()))
            .with_test_base_url(&format!("{}/siteverify", server.uri()));
    assert_eq!(
        unconfigured.verify(&check("token")).await,
        ProviderResult::configuration_invalid(ReasonCode::NotConfigured, TransportClass::None)
    );
    assert_eq!(
        unconfigured.probe_secret().await,
        ProviderResult::configuration_invalid(ReasonCode::NotConfigured, TransportClass::None)
    );
    let disabled = HttpTurnstileVerifier::new(TurnstileSettings::from_source(
        &turnstile_env().with("REGISTRATION_TURNSTILE_ENABLED", "false"),
    ))
    .with_test_base_url(&format!("{}/siteverify", server.uri()));
    assert_eq!(
        disabled.verify(&check("token")).await.reason,
        ReasonCode::Disabled
    );
}

#[tokio::test]
async fn negative_probe_confirms_the_secret_but_is_never_a_business_success() {
    // secret が受理され token は拒否される → Accepted (operational の根拠にならない)。
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_string_contains("response=emumet-readiness-probe"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(failure_body(&["invalid-input-response"])),
        )
        .mount(&server)
        .await;
    assert_eq!(
        verifier(&server).probe_secret().await,
        ProviderResult::accepted(TransportClass::Http2xx)
    );

    // 公式テスト secret のように任意の token を成功させる provider でも、probe は business success にしない。
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string(success_body(
            HOSTNAME,
            None,
            &now_ts(),
        )))
        .mount(&server)
        .await;
    let result = verifier(&server).probe_secret().await;
    assert_eq!(result.outcome, ProviderOutcome::Accepted);

    // secret 不正・一時障害はそのまま検出する。
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(400).set_body_string(failure_body(&["invalid-input-secret"])),
        )
        .mount(&server)
        .await;
    assert_eq!(
        verifier(&server).probe_secret().await.outcome,
        ProviderOutcome::ConfigurationInvalid
    );
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    assert_eq!(
        verifier(&server).probe_secret().await.outcome,
        ProviderOutcome::TemporarilyUnavailable
    );
}

/// 公式の公開テストキー (Cloudflare 公式ドキュメントの dummy secret/token) で実 Siteverify を呼ぶ。
/// 本番 credential の ready 証明ではなく、protocol と分類の実検証だけが目的。外部ネットワークが必要なので
/// 通常は実行しない: `cargo test -p driver live_turnstile -- --ignored`
#[tokio::test]
#[ignore = "calls the real Cloudflare Siteverify endpoint with the official public test keys (needs network)"]
async fn live_turnstile_official_test_keys() {
    let settings = |secret: &str| {
        TurnstileSettings::from_source(
            &EnvSource::new()
                .with("REGISTRATION_TURNSTILE_SECRET", secret)
                .with("REGISTRATION_TURNSTILE_EXPECTED_HOSTNAME", "example.com"),
        )
    };
    let dummy = "XXXX.DUMMY.TOKEN.XXXX";
    let pass = HttpTurnstileVerifier::new(settings("1x0000000000000000000000000000000AA"))
        .with_network_settings(TurnstileNetworkSettings {
            connect_timeout: Duration::from_secs(5),
            total_timeout: Duration::from_secs(10),
            max_response_bytes: 16 * 1024,
        });
    assert_eq!(
        pass.verify(&check(dummy)).await,
        ProviderResult::business_success(TransportClass::Http2xx)
    );
    assert_eq!(pass.probe_secret().await.outcome, ProviderOutcome::Accepted);
    let reject = HttpTurnstileVerifier::new(settings("2x0000000000000000000000000000000AA"));
    assert_eq!(
        reject.verify(&check(dummy)).await,
        ProviderResult::attempt_rejected(ReasonCode::TokenRejected, TransportClass::Http2xx)
    );
    let duplicate = HttpTurnstileVerifier::new(settings("3x0000000000000000000000000000000AA"));
    assert_eq!(
        duplicate.verify(&check(dummy)).await,
        ProviderResult::attempt_rejected(ReasonCode::TokenDuplicate, TransportClass::Http2xx)
    );
    let invalid = HttpTurnstileVerifier::new(settings("not-a-real-secret-fixture"));
    assert_eq!(
        invalid.verify(&check(dummy)).await,
        ProviderResult::configuration_invalid(ReasonCode::InvalidSecret, TransportClass::Http4xx)
    );
    assert_eq!(
        invalid.probe_secret().await.outcome,
        ProviderOutcome::ConfigurationInvalid
    );
    assert_eq!(
        pass.verify(&check("")).await.outcome,
        ProviderOutcome::AttemptRejected,
        "an empty token is a user error, not a provider failure"
    );
}

// ---- SMTP: 分類 -------------------------------------------------------------------------------

#[test]
fn smtp_failures_are_classified_by_stage_without_conflating_all_5xx() {
    let at = |stage, code: u16| SmtpFailure {
        stage,
        code: Some(code),
        ..SmtpFailure::default()
    };
    let transient = ProviderResult::temporarily_unavailable(
        ReasonCode::SmtpTransientFailure,
        TransportClass::SmtpTransient,
    );
    let auth = ProviderResult::configuration_invalid(
        ReasonCode::SmtpAuthFailed,
        TransportClass::SmtpAuthRejected,
    );
    let permanent = ProviderResult::configuration_invalid(
        ReasonCode::SmtpPermanentFailure,
        TransportClass::SmtpPermanent,
    );
    let recipient = ProviderResult::attempt_rejected(
        ReasonCode::RecipientRejected,
        TransportClass::SmtpPermanent,
    );
    let stages = [
        SmtpStage::Connect,
        SmtpStage::Auth,
        SmtpStage::MailFrom,
        SmtpStage::Rcpt,
        SmtpStage::Data,
    ];

    for stage in stages {
        // 4xx は段階によらず一時障害。認証・設定不正は configuration_invalid。
        for status in [421, 450, 451, 452] {
            assert_eq!(
                classify_smtp_failure(at(stage, status)),
                transient,
                "{stage:?} {status}"
            );
        }
        for status in [530, 534, 535, 538] {
            assert_eq!(
                classify_smtp_failure(at(stage, status)),
                auth,
                "{stage:?} {status}"
            );
        }
        // 認証以外の 5xx は宛先拒否ではなく設定不正。
        for status in [500, 501, 502, 503, 504, 521, 555] {
            assert_eq!(
                classify_smtp_failure(at(stage, status)),
                permanent,
                "{stage:?} {status}"
            );
        }
        // 宛先拒否になれるのは RCPT 段階の 550-554 だけ (MAIL FROM の「送信者不許可」・DATA・接続・認証の
        // 550 系は readiness を変えない attempt_rejected にせず、設定不正として扱う)。
        for status in [550, 551, 552, 553, 554] {
            let expected = if stage == SmtpStage::Rcpt {
                recipient
            } else {
                permanent
            };
            assert_eq!(
                classify_smtp_failure(at(stage, status)),
                expected,
                "{stage:?} {status}"
            );
        }
    }
    assert_eq!(
        classify_smtp_failure(SmtpFailure::default()),
        ProviderResult::temporarily_unavailable(
            ReasonCode::ConnectFailed,
            TransportClass::ConnectFailed
        )
    );
    assert_eq!(
        classify_smtp_failure(SmtpFailure {
            timeout: true,
            ..SmtpFailure::default()
        }),
        ProviderResult::temporarily_unavailable(ReasonCode::Timeout, TransportClass::Timeout)
    );
    assert_eq!(
        classify_smtp_failure(SmtpFailure {
            tls: true,
            ..SmtpFailure::default()
        }),
        ProviderResult::configuration_invalid(
            ReasonCode::SmtpTlsFailure,
            TransportClass::TlsFailed
        )
    );
    assert_eq!(
        classify_smtp_failure(SmtpFailure {
            client: true,
            ..SmtpFailure::default()
        })
        .outcome,
        ProviderOutcome::ConfigurationInvalid
    );
}

// ---- SMTP: ローカル SMTP 模擬への実送信 -----------------------------------------------------------

async fn transport_for(server: &MockSmtp) -> super::SmtpEmailTransport {
    SmtpEmailTransport::new(SmtpSettings::from_source_allowing_plaintext(&smtp_env(
        server.port(),
    )))
}

fn script_with_auth() -> SmtpScript {
    SmtpScript {
        credentials: Some(("mailer".to_string(), "smtp-password-0123456789".to_string())),
        ..SmtpScript::default()
    }
}

#[tokio::test]
async fn code_is_actually_sent_over_smtp_and_only_accepted_is_reported() {
    let server = MockSmtp::start(script_with_auth()).await;
    let transport = transport_for(&server).await;
    let result = transport.send_code("Alice@Example.com", "12345678").await;
    // SMTP 250 は受理 (Accepted) であって所有確認・business success ではない。
    assert_eq!(
        result,
        ProviderResult::accepted(TransportClass::SmtpAccepted)
    );
    let mails = server.mails();
    assert_eq!(mails.len(), 1);
    assert!(mails[0].from.contains("no-reply@example.com"));
    assert_eq!(mails[0].rcpt.len(), 1);
    assert!(
        mails[0].rcpt[0].contains("alice@example.com"),
        "{:?}",
        mails[0].rcpt
    );
    assert!(mails[0].data.contains("Subject: Emumet verification code"));
    assert_eq!(server.last_code(), "12345678");
}

#[tokio::test]
async fn smtp_authentication_failure_is_configuration_invalid() {
    let server = MockSmtp::start(SmtpScript {
        credentials: Some(("mailer".to_string(), "a-different-password".to_string())),
        ..SmtpScript::default()
    })
    .await;
    let transport = transport_for(&server).await;
    assert_eq!(
        transport.send_code("alice@example.com", "12345678").await,
        ProviderResult::configuration_invalid(
            ReasonCode::SmtpAuthFailed,
            TransportClass::SmtpAuthRejected
        )
    );
    assert!(server.mails().is_empty());
    assert_eq!(
        transport.probe_connection().await,
        ProviderResult::configuration_invalid(
            ReasonCode::SmtpAuthFailed,
            TransportClass::SmtpAuthRejected
        )
    );
}

#[tokio::test]
async fn smtp_recipient_rejection_is_an_attempt_rejection_not_a_provider_failure() {
    let server = MockSmtp::start(SmtpScript {
        rcpt_reply: "550 5.1.1 no such user".to_string(),
        ..script_with_auth()
    })
    .await;
    let transport = transport_for(&server).await;
    assert_eq!(
        transport.send_code("ghost@example.com", "12345678").await,
        ProviderResult::attempt_rejected(
            ReasonCode::RecipientRejected,
            TransportClass::SmtpPermanent
        )
    );
    assert!(server.mails().is_empty());
}

#[tokio::test]
async fn smtp_transient_and_permanent_failures_are_distinguished() {
    for (script, expected) in [
        (
            SmtpScript {
                rcpt_reply: "451 4.3.0 try again later".to_string(),
                ..script_with_auth()
            },
            ProviderOutcome::TemporarilyUnavailable,
        ),
        (
            SmtpScript {
                greeting: "421 4.3.2 service not available".to_string(),
                ..script_with_auth()
            },
            ProviderOutcome::TemporarilyUnavailable,
        ),
        (
            SmtpScript {
                data_reply: "452 4.2.2 mailbox full".to_string(),
                ..script_with_auth()
            },
            ProviderOutcome::TemporarilyUnavailable,
        ),
        (
            SmtpScript {
                mail_reply: "553 5.7.1 sender not allowed".to_string(),
                ..script_with_auth()
            },
            // MAIL FROM の拒否 (送信者不許可) は宛先拒否ではなく設定不正。
            ProviderOutcome::ConfigurationInvalid,
        ),
        (
            SmtpScript {
                mail_reply: "550 5.7.1 sender not authorized".to_string(),
                ..script_with_auth()
            },
            ProviderOutcome::ConfigurationInvalid,
        ),
        (
            SmtpScript {
                mail_reply: "530 5.7.0 authentication required".to_string(),
                ..script_with_auth()
            },
            ProviderOutcome::ConfigurationInvalid,
        ),
        (
            SmtpScript {
                rcpt_reply: "450 4.2.1 mailbox busy".to_string(),
                ..script_with_auth()
            },
            ProviderOutcome::TemporarilyUnavailable,
        ),
        (
            SmtpScript {
                rcpt_reply: "550 5.1.1 no such user".to_string(),
                ..script_with_auth()
            },
            ProviderOutcome::AttemptRejected,
        ),
        (
            SmtpScript {
                data_reply: "554 5.6.0 message rejected".to_string(),
                ..script_with_auth()
            },
            ProviderOutcome::ConfigurationInvalid,
        ),
        (
            SmtpScript {
                data_reply: "500 5.5.1 command unrecognized".to_string(),
                ..script_with_auth()
            },
            ProviderOutcome::ConfigurationInvalid,
        ),
    ] {
        let server = MockSmtp::start(script).await;
        let result = transport_for(&server)
            .await
            .send_code("alice@example.com", "12345678")
            .await;
        assert_eq!(result.outcome, expected, "{result:?}");
        assert!(server.mails().is_empty());
    }
}

#[tokio::test]
async fn smtp_connection_failure_and_timeout_are_temporarily_unavailable() {
    let closed = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let transport = SmtpEmailTransport::new(SmtpSettings::from_source_allowing_plaintext(
        &smtp_env(closed),
    ));
    assert_eq!(
        transport.send_code("alice@example.com", "12345678").await,
        ProviderResult::temporarily_unavailable(
            ReasonCode::ConnectFailed,
            TransportClass::ConnectFailed
        )
    );
    assert_eq!(
        transport.probe_connection().await.outcome,
        ProviderOutcome::TemporarilyUnavailable
    );

    let silent = MockSmtp::start(SmtpScript {
        silent: true,
        ..SmtpScript::default()
    })
    .await;
    let transport =
        SmtpEmailTransport::new(SmtpSettings::from_source_allowing_plaintext_with_timeout(
            &smtp_env(silent.port()),
            Duration::from_millis(400),
        ));
    let started = std::time::Instant::now();
    assert_eq!(
        transport.send_code("alice@example.com", "12345678").await,
        ProviderResult::temporarily_unavailable(ReasonCode::Timeout, TransportClass::Timeout)
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn starttls_is_required_so_a_server_without_it_is_a_configuration_failure() {
    let server = MockSmtp::start(script_with_auth()).await;
    let env = smtp_env(server.port()).with("REGISTRATION_SMTP_TLS", "starttls");
    let transport = SmtpEmailTransport::new(SmtpSettings::from_source_allowing_plaintext(&env));
    let result = transport.send_code("alice@example.com", "12345678").await;
    // 平文へ黙って降格して code を送らない。
    assert!(server.mails().is_empty());
    assert_eq!(
        result.outcome,
        ProviderOutcome::ConfigurationInvalid,
        "{result:?}"
    );
}

#[tokio::test]
async fn probe_checks_connection_and_auth_without_sending_mail() {
    let server = MockSmtp::start(script_with_auth()).await;
    let transport = transport_for(&server).await;
    assert_eq!(
        transport.probe_connection().await,
        ProviderResult::accepted(TransportClass::SmtpAccepted)
    );
    assert!(server.mails().is_empty());
    assert_eq!(
        transport.canary_address().as_deref(),
        Some("canary@example.com")
    );
}

#[tokio::test]
async fn unusable_smtp_configuration_never_connects() {
    let server = MockSmtp::start(SmtpScript::default()).await;
    let unconfigured = SmtpEmailTransport::new(SmtpSettings::from_source(&EnvSource::new()));
    assert_eq!(
        unconfigured.send_code("alice@example.com", "1").await,
        ProviderResult::configuration_invalid(ReasonCode::NotConfigured, TransportClass::None)
    );
    assert_eq!(
        unconfigured.probe_connection().await.reason,
        ReasonCode::NotConfigured
    );
    let production = SmtpEmailTransport::new(SmtpSettings::from_source(&smtp_env(server.port())));
    assert_eq!(
        production.send_code("alice@example.com", "1").await.reason,
        ReasonCode::InvalidConfiguration
    );
    assert_eq!(
        server.connections.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
}

#[tokio::test]
async fn malformed_recipients_are_rejected_before_any_connection() {
    let server = MockSmtp::start(script_with_auth()).await;
    let transport = transport_for(&server).await;
    for bad in [
        "alice@example.com\r\nRCPT TO:<victim@example.com>",
        "a@example.com, b@example.com",
        "not-an-address",
    ] {
        let result = transport.send_code(bad, "12345678").await;
        assert_eq!(result.outcome, ProviderOutcome::AttemptRejected, "{bad:?}");
    }
    assert_eq!(
        server.connections.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
}
