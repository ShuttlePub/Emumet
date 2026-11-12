//! 保護の経路 (確認コードの発行・検証・最終送信・bot token の検証・provider 障害・maintenance) は、
//! 登録者のメールアドレス・確認コード・email proof・bot token・送信元アドレスを tracing に出さない。
//! 実 PostgreSQL + fake provider。失敗・拒否・障害の経路を含めて全レベルのログを検査する。

use super::*;
use crate::service::registration_enforcement::{
    EnforceRegistrationUseCase, RegistrationAttempt, RegistrationVerdict,
};
use kernel::test_utils::LogCapture;

const APPLICANT: &str = "leak.applicant+case@example.com";
const OTHER_APPLICANT: &str = "second.leak.applicant@example.com";
const SOURCE: &str = "203.0.113.77";
const BOT_TOKEN: &str = "BotTokenLeakCheck-OOOO5555";
const BAD_BOT_TOKEN: &str = "RejectedBotTokenLeakCheck-PPPP6666";
const FLOW: &str = "89c3e9c6-9a10-4e1c-ae02-a04b53cc4166";

fn timeout_fault(at: OffsetDateTime) -> ProviderResult {
    ProviderResult::new(
        ProviderOutcome::TemporarilyUnavailable,
        ReasonCode::Timeout,
        TransportClass::Timeout,
    )
    .at_stage(CheckStage::Connect)
    .observed(at)
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn protection_flows_never_log_emails_codes_proofs_tokens_or_source_addresses() {
    let capture = LogCapture::start();
    let f = Fixture::new().await;
    f.ready_for_issuing().await;
    f.set_flow(FLOW, Duration::hours(1));

    let issued = f
        .module
        .issue_registration_email_challenge(&f.issue_request(APPLICANT, FLOW, SOURCE))
        .await
        .unwrap();
    let IssueOutcome::Issued { challenge_id, .. } = issued else {
        panic!("{issued:?}");
    };
    let (sent_to, code) = f.last_sent();
    assert_eq!(sent_to, APPLICANT.to_ascii_lowercase());
    let wrong = if code == "00000000" {
        "11111111"
    } else {
        "00000000"
    };
    for attempt in [wrong, "", "not-a-code"] {
        let outcome = f
            .module
            .confirm_registration_email_challenge(&challenge_id, attempt)
            .await
            .unwrap();
        assert!(matches!(outcome, ConfirmOutcome::Rejected));
    }
    let ConfirmOutcome::Confirmed { proof, .. } = f
        .module
        .confirm_registration_email_challenge(&challenge_id, &code)
        .await
        .unwrap()
    else {
        panic!("expected proof");
    };

    // 最終送信: 受理・bot token の不足・proof の不足・不正な proof。
    for (proof_value, token) in [
        (Some(proof.as_str()), Some(BOT_TOKEN)),
        (Some(proof.as_str()), None),
        (None, Some(BOT_TOKEN)),
        (Some("not-a-proof-QQQQ7777"), Some(BAD_BOT_TOKEN)),
    ] {
        let verdict = f
            .module
            .enforce_registration(RegistrationAttempt {
                flow_id: FLOW,
                email: APPLICANT,
                email_proof: proof_value,
                bot_token: token,
            })
            .await
            .unwrap();
        assert!(matches!(
            verdict,
            RegistrationVerdict::Accept | RegistrationVerdict::Reject
        ));
    }
    let _ = f
        .module
        .verify_registration_bot_token(BAD_BOT_TOKEN, Some(SOURCE))
        .await;

    // provider 障害 (メール送信・Siteverify の timeout) と、障害下の発行・検証・maintenance。
    let at = OffsetDateTime::now_utc();
    *f.module.email.send.lock().unwrap() = timeout_fault(at);
    *f.module.turnstile.verify.lock().unwrap() = timeout_fault(at);
    *f.module.email.probe.lock().unwrap() = timeout_fault(at);
    *f.module.turnstile.probe.lock().unwrap() = timeout_fault(at);
    let _ = f
        .module
        .issue_registration_email_challenge(&f.issue_request(OTHER_APPLICANT, FLOW, SOURCE))
        .await;
    let _ = f
        .module
        .verify_registration_bot_token(BOT_TOKEN, Some(SOURCE))
        .await;
    let _ = f
        .module
        .enforce_registration(RegistrationAttempt {
            flow_id: FLOW,
            email: OTHER_APPLICANT,
            email_proof: Some(proof.as_str()),
            bot_token: Some(BOT_TOKEN),
        })
        .await;
    let _ = f
        .module
        .run_registration_protection_maintenance(Duration::seconds(30))
        .await;

    capture.assert_sees("sqlx::query");
    capture.assert_absent(&[
        ("applicant email", APPLICANT),
        ("applicant local part", "leak.applicant"),
        ("second applicant email", OTHER_APPLICANT),
        ("confirmation code", code.as_str()),
        ("email proof", proof.as_str()),
        ("bot token", BOT_TOKEN),
        ("rejected bot token", BAD_BOT_TOKEN),
        ("source address", SOURCE),
    ]);
}
