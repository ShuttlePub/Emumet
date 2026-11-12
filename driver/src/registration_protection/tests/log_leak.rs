//! 保護 provider adapter (Turnstile / SMTP) は、成功・失敗・異常応答のいずれでも、provider の
//! secret・SMTP 認証情報・登録者のメールアドレス・確認コード・bot token を tracing に出さない。
//! provider の応答本文がそれらを echo しても出さないことを含む。

use super::smtp_mock::{MockSmtp, SmtpScript};
use super::*;
use kernel::test_utils::LogCapture;
use wiremock::matchers::any;

const TOKEN: &str = "BotTokenLeakCheck-0123456789";
const RECIPIENT: &str = "leak.recipient@example.com";
const CODE: &str = "98765432";
const SMTP_PASSWORD: &str = "smtp-password-0123456789";
const WRONG_PASSWORD: &str = "a-different-password-9876543210";

fn forbidden() -> Vec<(&'static str, &'static str)> {
    vec![
        ("turnstile secret", SECRET),
        ("bot token", TOKEN),
        ("recipient address", RECIPIENT),
        ("confirmation code", CODE),
        ("smtp password", SMTP_PASSWORD),
        ("wrong smtp password", WRONG_PASSWORD),
    ]
}

#[tokio::test]
async fn provider_adapters_never_log_credentials_addresses_codes_or_tokens() {
    let capture = LogCapture::start();
    tracing::info!("capture self-check");
    capture.assert_sees("capture self-check");

    for (status, body) in [
        (200, success_body(HOSTNAME, None, &now_ts())),
        (200, failure_body(&["invalid-input-response"])),
        (200, failure_body(&["invalid-input-secret"])),
        (500, format!("upstream error echoing {SECRET} and {TOKEN}")),
        (
            200,
            serde_json::json!({
                "success": false,
                "error-codes": ["internal-error"],
                "secret": SECRET,
                "response": TOKEN,
            })
            .to_string(),
        ),
        (200, "not json".to_string()),
    ] {
        let server = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(status).set_body_string(body))
            .mount(&server)
            .await;
        let verifier = verifier(&server);
        let _ = verifier.verify(&check(TOKEN)).await;
        let _ = verifier.probe_secret().await;
    }

    let scripts = [
        script_with_auth(),
        SmtpScript {
            credentials: Some(("mailer".to_string(), WRONG_PASSWORD.to_string())),
            ..SmtpScript::default()
        },
        SmtpScript {
            credentials: Some(("mailer".to_string(), SMTP_PASSWORD.to_string())),
            rcpt_reply: format!("550 no such user {RECIPIENT}"),
            ..SmtpScript::default()
        },
        SmtpScript {
            credentials: Some(("mailer".to_string(), SMTP_PASSWORD.to_string())),
            data_reply: format!("554 rejected {CODE} for {RECIPIENT}"),
            ..SmtpScript::default()
        },
        SmtpScript {
            credentials: Some(("mailer".to_string(), SMTP_PASSWORD.to_string())),
            mail_reply: format!("451 try later {SMTP_PASSWORD}"),
            ..SmtpScript::default()
        },
    ];
    for script in scripts {
        let server = MockSmtp::start(script).await;
        let transport = transport_for(&server).await;
        for purpose in [SendPurpose::Registration, SendPurpose::Canary] {
            let _ = transport.send_code(RECIPIENT, CODE, purpose).await;
        }
        let _ = transport.probe_connection().await;
    }

    capture.assert_absent(&forbidden());
}
