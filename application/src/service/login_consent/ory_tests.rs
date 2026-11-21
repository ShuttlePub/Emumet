//! The use cases on top of the real Hydra / Kratos clients against mocked Ory servers: what each
//! Ory failure turns into (a rejected login, or an error the route answers with 502).

use super::*;
use driver::hydra::HydraAdminClient;
use driver::kratos::KratosSessionClient;
use kernel::interfaces::login_consent::{
    ConsentChallenge, DependOnIdentitySessionVerifier, DependOnOAuth2FlowGateway, LoginChallenge,
    RedirectTarget, UpstreamErrorKind,
};
use serde_json::json;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct OryModule {
    hydra: HydraAdminClient,
    kratos: KratosSessionClient,
}

impl DependOnOAuth2FlowGateway for OryModule {
    type OAuth2FlowGateway = HydraAdminClient;
    fn oauth2_flow_gateway(&self) -> &HydraAdminClient {
        &self.hydra
    }
}

impl DependOnIdentitySessionVerifier for OryModule {
    type IdentitySessionVerifier = KratosSessionClient;
    fn identity_session_verifier(&self) -> &KratosSessionClient {
        &self.kratos
    }
}

fn module(hydra: &MockServer, kratos_url: &str) -> OryModule {
    OryModule {
        hydra: HydraAdminClient::new(hydra.uri()),
        kratos: KratosSessionClient::new(kratos_url.to_string()),
    }
}

async fn hydra_with_login_request() -> MockServer {
    let hydra = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/admin/oauth2/auth/requests/login"))
        .and(query_param("login_challenge", "login-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "challenge": "login-1",
            "skip": false,
            "subject": "",
            "client": null,
            "requested_scope": ["openid"],
            "requested_access_token_audience": ["account"],
            "request_url": "http://example.com"
        })))
        .mount(&hydra)
        .await;
    hydra
}

async fn mount_login_put(hydra: &MockServer, action: &str, response: ResponseTemplate) {
    Mock::given(method("PUT"))
        .and(path(format!("/admin/oauth2/auth/requests/login/{action}")))
        .and(query_param("login_challenge", "login-1"))
        .respond_with(response)
        .expect(1)
        .mount(hydra)
        .await;
}

fn redirect(url: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({ "redirect_to": url }))
}

async fn kratos_answering(response: ResponseTemplate) -> MockServer {
    let kratos = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/sessions/whoami"))
        .respond_with(response)
        .mount(&kratos)
        .await;
    kratos
}

#[tokio::test]
async fn every_kratos_failure_ends_in_a_rejected_login() {
    let answers = [
        ("401", ResponseTemplate::new(401)),
        (
            "inactive",
            ResponseTemplate::new(200).set_body_json(json!({
                "id": "s", "active": false, "identity": { "id": "i", "traits": {} }
            })),
        ),
        ("500", ResponseTemplate::new(500)),
        ("503", ResponseTemplate::new(503)),
        ("403", ResponseTemplate::new(403)),
        ("malformed", ResponseTemplate::new(200).set_body_string("{")),
    ];
    for (name, answer) in answers {
        let hydra = hydra_with_login_request().await;
        mount_login_put(&hydra, "reject", redirect("http://example.com/rejected")).await;
        let kratos = kratos_answering(answer).await;

        let result = module(&hydra, &kratos.uri())
            .login(
                &LoginChallenge::new("login-1"),
                "ory_kratos_session=tok; other=1",
            )
            .await;

        assert_eq!(
            result,
            Ok(RedirectTarget::new("http://example.com/rejected")),
            "{name}"
        );
    }
}

#[tokio::test]
async fn unreachable_kratos_ends_in_a_rejected_login() {
    let hydra = hydra_with_login_request().await;
    mount_login_put(&hydra, "reject", redirect("http://example.com/rejected")).await;

    let result = module(&hydra, "http://127.0.0.1:1")
        .login(&LoginChallenge::new("login-1"), "ory_kratos_session=tok")
        .await;

    assert_eq!(
        result,
        Ok(RedirectTarget::new("http://example.com/rejected"))
    );
}

#[tokio::test]
async fn only_the_session_cookie_reaches_kratos() {
    let hydra = hydra_with_login_request().await;
    mount_login_put(&hydra, "accept", redirect("http://example.com/next")).await;
    let kratos = kratos_answering(ResponseTemplate::new(200).set_body_json(json!({
        "id": "s", "active": true,
        "identity": { "id": "identity-1", "traits": { "email": "a@example.com" } }
    })))
    .await;

    let result = module(&hydra, &kratos.uri())
        .login(
            &LoginChallenge::new("login-1"),
            "tracking=abc; ory_kratos_session=tok; theme=dark",
        )
        .await;

    assert_eq!(result, Ok(RedirectTarget::new("http://example.com/next")));
    let requests = kratos.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let cookies: Vec<_> = requests[0].headers.get_all("cookie").iter().collect();
    assert_eq!(cookies.len(), 1);
    assert_eq!(cookies[0].to_str().unwrap(), "ory_kratos_session=tok");
}

#[tokio::test]
async fn hydra_failures_are_returned_as_upstream_errors() {
    // fetching the login request
    let hydra = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&hydra)
        .await;
    let error = module(&hydra, "http://127.0.0.1:1")
        .login(&LoginChallenge::new("login-1"), "ory_kratos_session=tok")
        .await
        .unwrap_err();
    let LoginConsentError::Upstream(error) = error else {
        panic!("expected an upstream error");
    };
    assert_eq!(error.kind(), UpstreamErrorKind::Status(404));

    // accepting the login
    let hydra = hydra_with_login_request().await;
    mount_login_put(&hydra, "accept", ResponseTemplate::new(500)).await;
    let kratos = kratos_answering(ResponseTemplate::new(200).set_body_json(json!({
        "id": "s", "active": true, "identity": { "id": "identity-1" }
    })))
    .await;
    let error = module(&hydra, &kratos.uri())
        .login(&LoginChallenge::new("login-1"), "ory_kratos_session=tok")
        .await
        .unwrap_err();
    let LoginConsentError::Upstream(error) = error else {
        panic!("expected an upstream error");
    };
    assert_eq!(error.kind(), UpstreamErrorKind::Status(500));

    // rejecting the login
    let hydra = hydra_with_login_request().await;
    mount_login_put(&hydra, "reject", ResponseTemplate::new(500)).await;
    let kratos = kratos_answering(ResponseTemplate::new(401)).await;
    let error = module(&hydra, &kratos.uri())
        .login(&LoginChallenge::new("login-1"), "ory_kratos_session=tok")
        .await
        .unwrap_err();
    assert!(matches!(error, LoginConsentError::Upstream(_)));

    // unreachable Hydra
    let hydra = MockServer::start().await;
    let module = OryModule {
        hydra: HydraAdminClient::new("http://127.0.0.1:1".to_string()),
        kratos: KratosSessionClient::new(hydra.uri()),
    };
    let error = module
        .begin_consent(&ConsentChallenge::new("consent-1"))
        .await
        .unwrap_err();
    let LoginConsentError::Upstream(error) = error else {
        panic!("expected an upstream error");
    };
    assert_eq!(error.kind(), UpstreamErrorKind::Unreachable);
}

#[tokio::test]
async fn consent_flow_through_the_hydra_client() {
    let hydra = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/admin/oauth2/auth/requests/consent"))
        .and(query_param("consent_challenge", "consent-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "challenge": "consent-1",
            "skip": false,
            "subject": "user",
            "client": { "client_name": "My App", "skip_consent": false },
            "requested_scope": ["openid", "email"],
            "requested_access_token_audience": ["account"],
            "context": { "email": "a@example.com" }
        })))
        .mount(&hydra)
        .await;
    Mock::given(method("PUT"))
        .and(path("/admin/oauth2/auth/requests/consent/accept"))
        .and(wiremock::matchers::body_json(json!({
            "grant_scope": ["openid", "email"],
            "grant_access_token_audience": ["account"],
            "remember": true,
            "remember_for": 3600,
            "session": { "id_token": { "email": "a@example.com" } }
        })))
        .respond_with(redirect("http://example.com/token"))
        .expect(1)
        .mount(&hydra)
        .await;
    let module = module(&hydra, "http://127.0.0.1:1");

    let outcome = module
        .begin_consent(&ConsentChallenge::new("consent-1"))
        .await
        .unwrap();
    let ConsentOutcome::Prompt(prompt) = outcome else {
        panic!("expected a prompt");
    };
    assert_eq!(prompt.client_name.as_deref(), Some("My App"));

    let redirect = module
        .decide_consent(ConsentDecision {
            challenge: ConsentChallenge::new("consent-1"),
            accept: true,
            grant_scope: Some(prompt.requested_scope),
        })
        .await
        .unwrap();
    assert_eq!(redirect, RedirectTarget::new("http://example.com/token"));

    let error = module
        .decide_consent(ConsentDecision {
            challenge: ConsentChallenge::new("consent-1"),
            accept: true,
            grant_scope: Some(vec!["admin".to_string()]),
        })
        .await
        .unwrap_err();
    assert_eq!(
        error,
        LoginConsentError::ScopeNotRequested("admin".to_string())
    );
}
