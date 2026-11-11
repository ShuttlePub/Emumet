//! 登録保護 adapter の contract test: 設定読込・設定世代・Turnstile Siteverify の分類 (E15b の実 API 応答を
//! fixture 化)・実 HTTP 経路 (wiremock) ・SMTP の実送信経路 (ローカル SMTP 模擬)。
//! 外部ネットワークを使うのは `#[ignore]` の live テストだけ (README / docs 参照)。

mod smtp_mock;

use super::config::{secret_fingerprint, ProcessEnv, ValueSource};
use super::smtp::{classify_smtp_failure, SmtpFailure, SmtpStage};
use super::*;
use kernel::interfaces::repository::{
    BotTokenCheck, EmailTransport, SendPurpose, TurnstileVerifier,
};
use kernel::prelude::entity::{
    CheckStage, ProviderOutcome, ProviderResult, ReasonCode, TransportClass,
};
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

// Evidence timestamps are asserted separately; these compare outcome, reason, transport and stage.
macro_rules! assert_provider_class {
    ($actual:expr, $expected:expr $(, $message:expr)? $(,)?) => {{
        let mut actual = $actual;
        let mut expected = $expected;
        actual.observed_at = None;
        expected.observed_at = None;
        assert_eq!(actual, expected $(, $message)?);
    }};
}

#[tokio::test]
async fn siteverify_success_and_negative_probe_use_exchange_start_but_faults_use_observation_time()
{
    for (probe, status) in [(false, 200), (true, 200), (false, 503)] {
        let server = MockServer::start().await;
        let body = if probe {
            failure_body(&["invalid-input-response"])
        } else {
            success_body(HOSTNAME, None, &now_ts())
        };
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(status)
                    .set_body_string(body)
                    .set_delay(Duration::from_millis(150)),
            )
            .mount(&server)
            .await;
        let verifier = HttpTurnstileVerifier::new(TurnstileSettings::from_source(&turnstile_env()))
            .with_test_base_url(&server.uri());
        let before = OffsetDateTime::now_utc();
        let task = tokio::spawn(async move {
            if probe {
                verifier.probe_secret().await
            } else {
                verifier
                    .verify(&BotTokenCheck {
                        token: "test",
                        remote_ip: None,
                    })
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while server.received_requests().await.unwrap().is_empty() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        let during = OffsetDateTime::now_utc();
        let result = task.await.unwrap();
        let evidence = result.observed_at.unwrap();
        assert!(evidence >= before);
        if status == 200 {
            assert!(evidence <= during);
            assert_eq!(
                result.outcome,
                if probe {
                    ProviderOutcome::Accepted
                } else {
                    ProviderOutcome::BusinessSuccess
                }
            );
        } else {
            assert!(evidence >= during);
            assert_eq!(result.outcome, ProviderOutcome::TemporarilyUnavailable);
        }
    }
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

/// 入力 token の無い管理者検証 (d15-q05) は、有効でないダミー token への応答で secret の有効性を判定する。
/// E15b の実応答 fixture: token 起因の拒否 = secret は有効 (Accepted になる attempt_rejected のまま)、
/// `invalid-input-secret` / `missing-input-secret` = 設定不正。HTTP 200 / 400 のどちらでも本文で判定する。
#[test]
fn the_negative_probe_decides_secret_validity_from_real_siteverify_responses() {
    let probe = |name: &str| {
        let (status, body) = fixture_case(name);
        classify_probe_response(status, &body, &expectation(), OffsetDateTime::now_utc())
    };
    for name in ["test_reject", "test_duplicate", "empty_response"] {
        assert_eq!(
            probe(name).outcome,
            ProviderOutcome::AttemptRejected,
            "{name}"
        );
    }
    assert_eq!(
        probe("invalid_secret"),
        ProviderResult::configuration_invalid(ReasonCode::InvalidSecret, TransportClass::Http4xx)
    );
    assert_eq!(
        probe("empty_secret"),
        ProviderResult::configuration_invalid(ReasonCode::MissingSecret, TransportClass::Http4xx)
    );
    // success:false の HTTP 200 でも、secret の有効性を示さない応答 (bad-request だけ・未知の code・本文なし) は
    // 準備済みの根拠にならない。
    for body in [
        failure_body(&["bad-request"]),
        failure_body(&[]),
        failure_body(&["something-new"]),
        "not json".to_string(),
        "{}".to_string(),
    ] {
        let result = classify_probe_response(
            200,
            body.as_bytes(),
            &expectation(),
            OffsetDateTime::now_utc(),
        );
        assert_eq!(
            result.outcome,
            ProviderOutcome::TemporarilyUnavailable,
            "{body}"
        );
    }
    // 一時障害はそのまま。
    for (status, outcome) in [
        (429, ProviderOutcome::TemporarilyUnavailable),
        (503, ProviderOutcome::TemporarilyUnavailable),
    ] {
        assert_eq!(
            classify_probe_response(status, b"", &expectation(), OffsetDateTime::now_utc()).outcome,
            outcome
        );
    }
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
    assert_provider_class!(
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
    assert_provider_class!(
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
        assert_provider_class!(
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
    assert_provider_class!(
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
    assert_provider_class!(
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
    assert_provider_class!(
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
    assert_provider_class!(
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
    assert_provider_class!(
        unconfigured.verify(&check("token")).await,
        ProviderResult::configuration_invalid(ReasonCode::NotConfigured, TransportClass::None)
    );
    assert_provider_class!(
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
    assert_provider_class!(
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

    // bad-request だけの応答は secret の有効性を示さないので、準備済みの根拠にしない。
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string(failure_body(&["bad-request"])))
        .mount(&server)
        .await;
    assert_eq!(
        verifier(&server).probe_secret().await.outcome,
        ProviderOutcome::TemporarilyUnavailable
    );
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
    assert_provider_class!(
        pass.verify(&check(dummy)).await,
        ProviderResult::business_success(TransportClass::Http2xx)
    );
    assert_eq!(pass.probe_secret().await.outcome, ProviderOutcome::Accepted);
    let reject = HttpTurnstileVerifier::new(settings("2x0000000000000000000000000000000AA"));
    // 入力 token の無い検証 (有効でないダミー token): 有効な secret は invalid-input-response で
    // token だけが拒否される → secret 有効 (Accepted。管理者検証で business success に昇格する)。
    assert_eq!(
        reject.probe_secret().await.outcome,
        ProviderOutcome::Accepted,
        "official always-fail key: the secret is valid, only the dummy token is rejected"
    );
    assert_eq!(
        HttpTurnstileVerifier::new(settings("3x0000000000000000000000000000000AA"))
            .probe_secret()
            .await
            .outcome,
        ProviderOutcome::Accepted
    );
    assert_provider_class!(
        reject.verify(&check(dummy)).await,
        ProviderResult::attempt_rejected(ReasonCode::TokenRejected, TransportClass::Http2xx)
    );
    let duplicate = HttpTurnstileVerifier::new(settings("3x0000000000000000000000000000000AA"));
    assert_provider_class!(
        duplicate.verify(&check(dummy)).await,
        ProviderResult::attempt_rejected(ReasonCode::TokenDuplicate, TransportClass::Http2xx)
    );
    let invalid = HttpTurnstileVerifier::new(settings("not-a-real-secret-fixture"));
    assert_provider_class!(
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

/// 分類 (outcome / reason / transport) だけを取り出す。証拠 (段階・観測時刻) は別のテストで検証する。
fn kind(result: ProviderResult) -> ProviderResult {
    ProviderResult::new(result.outcome, result.reason, result.transport)
}

fn classify(failure: SmtpFailure) -> ProviderResult {
    kind(classify_smtp_failure(failure, SendPurpose::Registration))
}

fn classify_canary(failure: SmtpFailure) -> ProviderResult {
    kind(classify_smtp_failure(failure, SendPurpose::Canary))
}

#[test]
fn smtp_failures_are_classified_by_stage_and_nothing_after_the_recipient_changes_readiness() {
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
    // 宛先を提示する前 (接続・認証・MAIL FROM) だけが provider の障害を示せる。
    for stage in [SmtpStage::Connect, SmtpStage::Auth, SmtpStage::MailFrom] {
        for status in [421, 450, 451, 452] {
            assert_eq!(classify(at(stage, status)), transient, "{stage:?} {status}");
        }
        for status in [530, 534, 535, 538] {
            assert_eq!(classify(at(stage, status)), auth, "{stage:?} {status}");
        }
        for status in [500, 501, 502, 503, 504, 521, 550, 551, 552, 553, 554, 555] {
            assert_eq!(classify(at(stage, status)), permanent, "{stage:?} {status}");
        }
    }
    // RCPT コマンド送信以降は、どのコードでも attempt_rejected (readiness の根拠にしない)。
    for stage in [SmtpStage::Rcpt, SmtpStage::Data] {
        for status in [
            421, 450, 451, 452, 500, 501, 503, 530, 535, 550, 551, 552, 553, 554, 555,
        ] {
            let result = classify(at(stage, status));
            assert_eq!(
                result.outcome,
                ProviderOutcome::AttemptRejected,
                "{stage:?} {status}"
            );
            let expected = if stage == SmtpStage::Rcpt && (550..=554).contains(&status) {
                ReasonCode::RecipientRejected
            } else {
                ReasonCode::RecipientDeferred
            };
            assert_eq!(result.reason, expected, "{stage:?} {status}");
        }
        // timeout・切断・client/tls の各種もその段階では attempt レベル。
        for failure in [
            SmtpFailure {
                stage,
                timeout: true,
                ..SmtpFailure::default()
            },
            SmtpFailure {
                stage,
                ..SmtpFailure::default()
            },
            SmtpFailure {
                stage,
                client: true,
                ..SmtpFailure::default()
            },
            SmtpFailure {
                stage,
                tls: true,
                ..SmtpFailure::default()
            },
        ] {
            assert_eq!(
                classify(failure).outcome,
                ProviderOutcome::AttemptRejected,
                "{failure:?}"
            );
        }
    }
    // 宛先提示前の接続失敗・timeout は一時障害、TLS・client の不備は設定不正。
    for stage in [SmtpStage::Connect, SmtpStage::Auth, SmtpStage::MailFrom] {
        assert_eq!(
            classify(SmtpFailure {
                stage,
                ..SmtpFailure::default()
            }),
            ProviderResult::temporarily_unavailable(
                ReasonCode::ConnectFailed,
                TransportClass::ConnectFailed
            )
        );
        assert_eq!(
            classify(SmtpFailure {
                stage,
                timeout: true,
                ..SmtpFailure::default()
            }),
            ProviderResult::temporarily_unavailable(ReasonCode::Timeout, TransportClass::Timeout)
        );
        assert_eq!(
            classify(SmtpFailure {
                stage,
                tls: true,
                ..SmtpFailure::default()
            }),
            ProviderResult::configuration_invalid(
                ReasonCode::SmtpTlsFailure,
                TransportClass::TlsFailed
            )
        );
        assert_eq!(
            classify(SmtpFailure {
                stage,
                client: true,
                ..SmtpFailure::default()
            })
            .outcome,
            ProviderOutcome::ConfigurationInvalid
        );
    }
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
    let result = transport
        .send_code("Alice@Example.com", "12345678", SendPurpose::Registration)
        .await;
    // SMTP 250 は受理 (Accepted) であって所有確認・business success ではない。
    assert_eq!(
        kind(result),
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
        kind(
            transport
                .send_code("alice@example.com", "12345678", SendPurpose::Registration)
                .await
        ),
        ProviderResult::configuration_invalid(
            ReasonCode::SmtpAuthFailed,
            TransportClass::SmtpAuthRejected
        )
    );
    assert!(server.mails().is_empty());
    assert_eq!(
        kind(transport.probe_connection().await),
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
        kind(
            transport
                .send_code("ghost@example.com", "12345678", SendPurpose::Registration)
                .await
        ),
        ProviderResult::attempt_rejected(
            ReasonCode::RecipientRejected,
            TransportClass::SmtpPermanent
        )
    );
    assert!(server.mails().is_empty());
}

async fn outcome_of(script: SmtpScript, timeout: Duration) -> (ProviderResult, usize) {
    let server = MockSmtp::start(script).await;
    let transport =
        SmtpEmailTransport::new(SmtpSettings::from_source_allowing_plaintext_with_timeout(
            &smtp_env(server.port()),
            timeout,
        ));
    let result = transport
        .send_code("alice@example.com", "12345678", SendPurpose::Registration)
        .await;
    (result, server.mails().len())
}

#[tokio::test]
async fn failures_before_the_recipient_is_presented_can_signal_a_provider_problem() {
    let long = Duration::from_secs(5);
    for (script, expected) in [
        (
            SmtpScript {
                greeting: "421 4.3.2 service not available".to_string(),
                ..script_with_auth()
            },
            ProviderOutcome::TemporarilyUnavailable,
        ),
        (
            SmtpScript {
                mail_reply: "450 4.2.1 sender temporarily rejected".to_string(),
                ..script_with_auth()
            },
            ProviderOutcome::TemporarilyUnavailable,
        ),
        // MAIL FROM の拒否 (送信者不許可・認証要求) は宛先拒否ではなく設定不正。
        (
            SmtpScript {
                mail_reply: "553 5.7.1 sender not allowed".to_string(),
                ..script_with_auth()
            },
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
    ] {
        let (result, mails) = outcome_of(script, long).await;
        assert_eq!(result.outcome, expected, "{result:?}");
        assert_eq!(mails, 0);
    }
    // MAIL FROM で固まる (宛先提示前の timeout) は一時障害。
    let (result, _) = outcome_of(
        SmtpScript {
            silent_on: Some("MAIL FROM".to_string()),
            ..script_with_auth()
        },
        Duration::from_millis(400),
    )
    .await;
    assert_eq!(
        kind(result),
        ProviderResult::temporarily_unavailable(ReasonCode::Timeout, TransportClass::Timeout)
    );
    assert_eq!(
        result.stage,
        Some(CheckStage::MailFrom),
        "the overall timeout is attributed to the last started stage"
    );
}

#[tokio::test]
async fn every_failure_after_the_recipient_is_presented_is_an_attempt_level_failure() {
    // RCPT 以降の失敗は、コード・段階・timeout・切断のいずれでも provider の健全性の根拠にしない
    // (readiness を変えると後続の公開応答から先行宛先の状態が判別できる)。
    let long = Duration::from_secs(5);
    for reply in [
        "421 4.3.2 shutting down",
        "450 4.2.1 mailbox busy",
        "451 4.3.0 try again later",
        "452 4.2.2 mailbox full",
        "500 5.5.1 command unrecognized",
        "503 5.5.1 bad sequence",
        "550 5.1.1 no such user",
        "554 5.7.1 rejected",
    ] {
        let (result, mails) = outcome_of(
            SmtpScript {
                rcpt_reply: reply.to_string(),
                ..script_with_auth()
            },
            long,
        )
        .await;
        assert_eq!(
            result.outcome,
            ProviderOutcome::AttemptRejected,
            "RCPT {reply}: {result:?}"
        );
        assert_eq!(mails, 0);
    }
    for reply in [
        "421 4.3.2 shutting down",
        "451 4.3.0 try again later",
        "452 4.2.2 mailbox full",
        "500 5.5.1 command unrecognized",
        "554 5.6.0 message rejected",
    ] {
        let (result, mails) = outcome_of(
            SmtpScript {
                data_reply: reply.to_string(),
                ..script_with_auth()
            },
            long,
        )
        .await;
        assert_eq!(
            result.outcome,
            ProviderOutcome::AttemptRejected,
            "DATA {reply}: {result:?}"
        );
        assert_eq!(mails, 0);
    }
    let short = Duration::from_millis(400);
    for script in [
        SmtpScript {
            silent_on: Some("RCPT TO".to_string()),
            ..script_with_auth()
        },
        SmtpScript {
            silent_on: Some("DATA".to_string()),
            ..script_with_auth()
        },
        SmtpScript {
            silent_data_end: true,
            ..script_with_auth()
        },
        SmtpScript {
            close_on: Some("RCPT TO".to_string()),
            ..script_with_auth()
        },
        SmtpScript {
            close_on: Some("DATA".to_string()),
            ..script_with_auth()
        },
    ] {
        let label = format!("{script:?}");
        let (result, _) = outcome_of(script, short).await;
        assert_eq!(
            result.outcome,
            ProviderOutcome::AttemptRejected,
            "{label}: {result:?}"
        );
    }
    // 550 は宛先拒否、それ以外は宛先提示後の失敗として区別する (どちらも観測は変えない)。
    let (rejected, _) = outcome_of(
        SmtpScript {
            rcpt_reply: "550 5.1.1 no such user".to_string(),
            ..script_with_auth()
        },
        long,
    )
    .await;
    assert_eq!(rejected.reason, ReasonCode::RecipientRejected);
    let (deferred, _) = outcome_of(
        SmtpScript {
            rcpt_reply: "450 4.2.1 mailbox busy".to_string(),
            ..script_with_auth()
        },
        long,
    )
    .await;
    assert_eq!(deferred.reason, ReasonCode::RecipientDeferred);
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
        kind(
            transport
                .send_code("alice@example.com", "12345678", SendPurpose::Registration)
                .await
        ),
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
        kind(
            transport
                .send_code("alice@example.com", "12345678", SendPurpose::Registration)
                .await
        ),
        ProviderResult::temporarily_unavailable(ReasonCode::Timeout, TransportClass::Timeout)
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn starttls_is_required_so_a_server_without_it_is_a_configuration_failure() {
    let server = MockSmtp::start(script_with_auth()).await;
    let env = smtp_env(server.port()).with("REGISTRATION_SMTP_TLS", "starttls");
    let transport = SmtpEmailTransport::new(SmtpSettings::from_source_allowing_plaintext(&env));
    let result = transport
        .send_code("alice@example.com", "12345678", SendPurpose::Registration)
        .await;
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
        kind(transport.probe_connection().await),
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
        kind(
            unconfigured
                .send_code("alice@example.com", "1", SendPurpose::Registration)
                .await
        ),
        ProviderResult::configuration_invalid(ReasonCode::NotConfigured, TransportClass::None)
    );
    assert_eq!(
        unconfigured.probe_connection().await.reason,
        ReasonCode::NotConfigured
    );
    let production = SmtpEmailTransport::new(SmtpSettings::from_source(&smtp_env(server.port())));
    assert_eq!(
        production
            .send_code("alice@example.com", "1", SendPurpose::Registration)
            .await
            .reason,
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
        let result = transport
            .send_code(bad, "12345678", SendPurpose::Registration)
            .await;
        assert_eq!(result.outcome, ProviderOutcome::AttemptRejected, "{bad:?}");
    }
    assert_eq!(
        server.connections.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
}

#[tokio::test]
async fn a_silent_quit_does_not_override_a_delivery_the_server_already_accepted() {
    let server = MockSmtp::start(SmtpScript {
        silent_quit: true,
        ..script_with_auth()
    })
    .await;
    let transport =
        SmtpEmailTransport::new(SmtpSettings::from_source_allowing_plaintext_with_timeout(
            &smtp_env(server.port()),
            Duration::from_millis(400),
        ));
    let started = std::time::Instant::now();
    // DATA が受理された後に QUIT へ応答がなくても、送信は成功のまま (全体 timeout に覆されない)。
    assert_eq!(
        kind(
            transport
                .send_code("alice@example.com", "12345678", SendPurpose::Registration)
                .await
        ),
        ProviderResult::accepted(TransportClass::SmtpAccepted)
    );
    assert_eq!(server.mails().len(), 1);
    assert!(started.elapsed() < Duration::from_secs(6));
    // probe も同様 (NOOP 成功の後の QUIT 無応答で結果が変わらない)。
    assert_eq!(
        kind(transport.probe_connection().await),
        ProviderResult::accepted(TransportClass::SmtpAccepted)
    );
}

#[test]
fn canary_delivery_failures_are_provider_failures_at_every_stage() {
    // 管理者 canary は固定の承認済み宛先なので、RCPT・DATA・timeout・切断も provider の障害として分類する
    // (公開要求ではこれらを attempt_rejected にして観測から除外する)。
    let at = |stage, code: u16| SmtpFailure {
        stage,
        code: Some(code),
        ..SmtpFailure::default()
    };
    for stage in [SmtpStage::Rcpt, SmtpStage::Data] {
        for status in [421, 450, 451, 452] {
            assert_eq!(
                classify_canary(at(stage, status)),
                ProviderResult::temporarily_unavailable(
                    ReasonCode::SmtpTransientFailure,
                    TransportClass::SmtpTransient
                ),
                "{stage:?} {status}"
            );
            assert_eq!(
                classify(at(stage, status)).outcome,
                ProviderOutcome::AttemptRejected,
                "the same failure is excluded from the observation for a public request"
            );
        }
        for status in [500, 503, 550, 551, 552, 553, 554, 555] {
            assert_eq!(
                classify_canary(at(stage, status)),
                ProviderResult::configuration_invalid(
                    ReasonCode::SmtpPermanentFailure,
                    TransportClass::SmtpPermanent
                ),
                "{stage:?} {status}"
            );
        }
        assert_eq!(
            classify_canary(SmtpFailure {
                stage,
                timeout: true,
                ..SmtpFailure::default()
            }),
            ProviderResult::temporarily_unavailable(ReasonCode::Timeout, TransportClass::Timeout)
        );
        assert_eq!(
            classify_canary(SmtpFailure {
                stage,
                ..SmtpFailure::default()
            }),
            ProviderResult::temporarily_unavailable(
                ReasonCode::ConnectFailed,
                TransportClass::ConnectFailed
            )
        );
    }
}

#[tokio::test]
async fn a_canary_delivery_failure_after_the_recipient_is_a_provider_failure_over_real_smtp() {
    let canary = |script: SmtpScript, timeout: Duration| async move {
        let server = MockSmtp::start(script).await;
        let transport =
            SmtpEmailTransport::new(SmtpSettings::from_source_allowing_plaintext_with_timeout(
                &smtp_env(server.port()),
                timeout,
            ));
        transport
            .send_code("canary@example.com", "12345678", SendPurpose::Canary)
            .await
    };
    let long = Duration::from_secs(5);
    for (script, expected) in [
        (
            SmtpScript {
                data_reply: "452 4.2.2 mailbox full".to_string(),
                ..script_with_auth()
            },
            ProviderOutcome::TemporarilyUnavailable,
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
            ProviderOutcome::ConfigurationInvalid,
        ),
        (
            SmtpScript {
                data_reply: "554 5.6.0 message rejected".to_string(),
                ..script_with_auth()
            },
            ProviderOutcome::ConfigurationInvalid,
        ),
    ] {
        let result = canary(script, long).await;
        assert_eq!(result.outcome, expected, "{result:?}");
    }
    // RCPT 後に固まる・切断される配送も provider の一時障害。
    for script in [
        SmtpScript {
            silent_on: Some("RCPT TO".to_string()),
            ..script_with_auth()
        },
        SmtpScript {
            silent_data_end: true,
            ..script_with_auth()
        },
        SmtpScript {
            close_on: Some("DATA".to_string()),
            ..script_with_auth()
        },
    ] {
        let label = format!("{script:?}");
        let result = canary(script, Duration::from_millis(400)).await;
        assert_eq!(
            result.outcome,
            ProviderOutcome::TemporarilyUnavailable,
            "{label}: {result:?}"
        );
    }
    // 受理された canary は従来どおり Accepted (business success は code の確認 roundtrip だけ)。
    let accepted = canary(script_with_auth(), long).await;
    assert_eq!(
        kind(accepted),
        ProviderResult::accepted(TransportClass::SmtpAccepted)
    );
}

async fn transport_with(
    script: SmtpScript,
    server_timeout: Duration,
) -> (SmtpEmailTransport, MockSmtp) {
    let server = MockSmtp::start(script).await;
    let transport =
        SmtpEmailTransport::new(SmtpSettings::from_source_allowing_plaintext_with_timeout(
            &smtp_env(server.port()),
            server_timeout,
        ));
    (transport, server)
}

#[test]
fn every_smtp_stage_maps_to_the_evidence_stage_it_failed_at() {
    for (stage, expected) in [
        (SmtpStage::Connect, CheckStage::Connect),
        (SmtpStage::StartTls, CheckStage::StartTls),
        (SmtpStage::Auth, CheckStage::Auth),
        (SmtpStage::MailFrom, CheckStage::MailFrom),
        (SmtpStage::Rcpt, CheckStage::Rcpt),
        (SmtpStage::Data, CheckStage::Data),
    ] {
        for code in [421, 535, 550] {
            let failure = SmtpFailure {
                stage,
                code: Some(code),
                ..SmtpFailure::default()
            };
            let canary = classify_smtp_failure(failure, SendPurpose::Canary);
            assert_eq!(canary.stage, Some(expected), "{stage:?} {code}");
            let public = classify_smtp_failure(failure, SendPurpose::Registration);
            if public.outcome == ProviderOutcome::AttemptRejected {
                // 宛先提示後の公開要求の失敗は記録されないので、証拠も持たない。
                assert!(matches!(stage, SmtpStage::Rcpt | SmtpStage::Data));
                assert_eq!(public.stage, None);
            } else {
                assert_eq!(public.stage, Some(expected), "{stage:?} {code}");
            }
        }
    }
}

#[tokio::test]
async fn a_delivery_success_carries_the_data_acceptance_time_as_its_evidence() {
    let before = OffsetDateTime::now_utc();
    let (transport, server) = transport_with(script_with_auth(), Duration::from_secs(5)).await;
    let result = transport
        .send_code("canary@example.com", "12345678", SendPurpose::Canary)
        .await;
    let after = OffsetDateTime::now_utc();
    assert_eq!(result.outcome, ProviderOutcome::Accepted);
    assert_eq!(result.stage, Some(CheckStage::Data));
    let observed = result
        .observed_at
        .expect("the exchange time is the evidence");
    assert!(before <= observed && observed <= after);
    assert_eq!(server.mails().len(), 1);
}

#[tokio::test]
async fn the_probe_verifies_mail_from_without_presenting_a_recipient() {
    let before = OffsetDateTime::now_utc();
    let (transport, server) = transport_with(script_with_auth(), Duration::from_secs(5)).await;
    let result = transport.probe_connection().await;
    let after = OffsetDateTime::now_utc();
    assert_eq!(result.outcome, ProviderOutcome::Accepted);
    // MAIL FROM まで覆うが、RCPT・DATA の障害は覆わない。
    assert_eq!(result.stage, Some(CheckStage::MailFrom));
    let observed = result
        .observed_at
        .expect("the exchange time is the evidence");
    assert!(before <= observed && observed <= after);
    assert!(server.mails().is_empty());
    assert!(server.commands().iter().any(|c| c.starts_with("MAIL FROM")));
    assert!(server.commands().iter().any(|c| c == "RSET"));
    assert!(
        !server
            .commands()
            .iter()
            .any(|c| c.starts_with("RCPT TO") || c == "DATA"),
        "the probe never presents a recipient: {:?}",
        server.commands()
    );
}

#[tokio::test]
async fn a_probe_that_cannot_pass_mail_from_is_a_failure_at_the_mail_from_stage() {
    for (reply, outcome) in [
        (
            "550 5.7.1 sender not allowed",
            ProviderOutcome::ConfigurationInvalid,
        ),
        (
            "451 4.3.0 try later",
            ProviderOutcome::TemporarilyUnavailable,
        ),
    ] {
        let (transport, _server) = transport_with(
            SmtpScript {
                mail_reply: reply.to_string(),
                ..script_with_auth()
            },
            Duration::from_secs(5),
        )
        .await;
        let result = transport.probe_connection().await;
        assert_eq!(result.outcome, outcome, "{reply}");
        assert_eq!(result.stage, Some(CheckStage::MailFrom), "{reply}");
        assert!(result.observed_at.is_some());
    }
}

#[tokio::test]
async fn failures_over_real_smtp_record_the_stage_they_failed_at() {
    let long = Duration::from_secs(5);
    // 認証失敗。
    let (transport, _server) = transport_with(
        SmtpScript {
            credentials: Some(("mailer".to_string(), "a-different-password".to_string())),
            ..SmtpScript::default()
        },
        long,
    )
    .await;
    let result = transport.probe_connection().await;
    assert_eq!(result.stage, Some(CheckStage::Auth));
    assert!(result.observed_at.is_some());

    // MAIL FROM 拒否 (送信)。
    let (transport, _server) = transport_with(
        SmtpScript {
            mail_reply: "550 5.7.1 sender not allowed".to_string(),
            ..script_with_auth()
        },
        long,
    )
    .await;
    let result = transport
        .send_code("alice@example.com", "12345678", SendPurpose::Registration)
        .await;
    assert_eq!(result.outcome, ProviderOutcome::ConfigurationInvalid);
    assert_eq!(result.stage, Some(CheckStage::MailFrom));

    // canary の RCPT 拒否・DATA 拒否。
    for (script, stage) in [
        (
            SmtpScript {
                rcpt_reply: "550 5.1.1 no such user".to_string(),
                ..script_with_auth()
            },
            CheckStage::Rcpt,
        ),
        (
            SmtpScript {
                data_reply: "452 4.2.2 mailbox full".to_string(),
                ..script_with_auth()
            },
            CheckStage::Data,
        ),
    ] {
        let (transport, _server) = transport_with(script, long).await;
        let result = transport
            .send_code("canary@example.com", "12345678", SendPurpose::Canary)
            .await;
        assert_eq!(result.stage, Some(stage));
    }

    // STARTTLS を提供しないサーバー。
    let server = MockSmtp::start(script_with_auth()).await;
    let env = smtp_env(server.port()).with("REGISTRATION_SMTP_TLS", "starttls");
    let transport = SmtpEmailTransport::new(SmtpSettings::from_source_allowing_plaintext(&env));
    let result = transport.probe_connection().await;
    assert_eq!(result.outcome, ProviderOutcome::ConfigurationInvalid);
    assert_eq!(result.stage, Some(CheckStage::StartTls));
}

#[tokio::test]
async fn a_silent_rset_does_not_turn_an_accepted_mail_from_into_a_timeout() {
    // MAIL FROM は期限内に受理されたが、その後の RSET に応答がない。確定済みの受理は
    // 全体の期限 (ここでは 400ms) の外で待つ RSET・QUIT に影響されない。
    let (transport, _server) = transport_with(
        SmtpScript {
            silent_on: Some("RSET".to_string()),
            ..script_with_auth()
        },
        Duration::from_millis(400),
    )
    .await;
    let result = transport.probe_connection().await;
    assert_eq!(result.outcome, ProviderOutcome::Accepted, "{result:?}");
    assert_eq!(result.stage, Some(CheckStage::MailFrom));
}
