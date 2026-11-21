use super::*;
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const UNREACHABLE: &str = "http://127.0.0.1:1";
const COOKIE: &str = "ory_kratos_session=test-session-token";

fn session_body(active: serde_json::Value, traits: serde_json::Value) -> serde_json::Value {
    json!({
        "id": "session-id",
        "active": active,
        "identity": { "id": "identity-uuid", "traits": traits }
    })
}

async fn whoami(response: ResponseTemplate) -> Result<Option<VerifiedSession>, UpstreamError> {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/sessions/whoami"))
        .respond_with(response)
        .mount(&server)
        .await;
    KratosSessionClient::new(server.uri())
        .verify_session(COOKIE)
        .await
}

#[tokio::test]
async fn active_session_is_verified_with_identity_id_and_email() {
    let session = whoami(
        ResponseTemplate::new(200)
            .set_body_json(session_body(json!(true), json!({"email": "a@example.com"}))),
    )
    .await
    .unwrap();

    assert_eq!(
        session,
        Some(VerifiedSession {
            identity_id: "identity-uuid".to_string(),
            email: Some("a@example.com".to_string()),
        })
    );
}

#[tokio::test]
async fn session_without_email_trait_has_no_email() {
    for traits in [json!({}), json!({"email": 3}), json!(null)] {
        let session =
            whoami(ResponseTemplate::new(200).set_body_json(session_body(json!(true), traits)))
                .await
                .unwrap()
                .unwrap();

        assert_eq!(session.identity_id, "identity-uuid");
        assert_eq!(session.email, None);
    }
}

#[tokio::test]
async fn only_the_given_session_cookie_is_forwarded() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/sessions/whoami"))
        .and(header("cookie", COOKIE))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(session_body(json!(true), json!({}))),
        )
        .expect(1)
        .mount(&server)
        .await;

    let session = KratosSessionClient::new(format!("{}/", server.uri()))
        .verify_session(COOKIE)
        .await
        .unwrap();

    assert!(session.is_some());
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let cookies: Vec<_> = requests[0].headers.get_all("cookie").iter().collect();
    assert_eq!(cookies.len(), 1);
    assert_eq!(cookies[0].to_str().unwrap(), COOKIE);
}

#[tokio::test]
async fn unauthorized_is_no_session() {
    let session = whoami(ResponseTemplate::new(401)).await.unwrap();

    assert_eq!(session, None);
}

#[tokio::test]
async fn inactive_session_is_no_session() {
    let explicit =
        whoami(ResponseTemplate::new(200).set_body_json(session_body(json!(false), json!({}))))
            .await
            .unwrap();
    let missing_flag = whoami(ResponseTemplate::new(200).set_body_json(json!({
        "id": "session-id",
        "identity": { "id": "identity-uuid" }
    })))
    .await
    .unwrap();

    assert_eq!(explicit, None);
    assert_eq!(missing_flag, None);
}

#[tokio::test]
async fn kratos_errors_other_than_401_are_status_errors() {
    for status in [403_u16, 404, 500, 502, 503] {
        let error = whoami(ResponseTemplate::new(status)).await.unwrap_err();

        assert_eq!(error.kind(), UpstreamErrorKind::Status(status));
    }
}

#[tokio::test]
async fn malformed_body_is_an_invalid_response() {
    let not_json = whoami(ResponseTemplate::new(200).set_body_string("not json"))
        .await
        .unwrap_err();
    let no_identity = whoami(ResponseTemplate::new(200).set_body_json(json!({"id": "s"})))
        .await
        .unwrap_err();

    assert_eq!(not_json.kind(), UpstreamErrorKind::InvalidResponse);
    assert_eq!(no_identity.kind(), UpstreamErrorKind::InvalidResponse);
}

#[tokio::test]
async fn connection_failure_is_unreachable() {
    let error = KratosSessionClient::new(UNREACHABLE.to_string())
        .verify_session(COOKIE)
        .await
        .unwrap_err();

    assert_eq!(error.kind(), UpstreamErrorKind::Unreachable);
}

#[test]
#[should_panic(expected = "KRATOS_PUBLIC_URL is not a valid URL")]
fn invalid_public_url_panics_at_construction() {
    let _ = KratosSessionClient::new("not a url".to_string());
}
