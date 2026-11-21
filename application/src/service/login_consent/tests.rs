//! The login / consent use cases against fake ports: no database, no HTTP.

use super::*;
use kernel::interfaces::login_consent::{
    AcceptConsent, AcceptLogin, ConsentChallenge, ConsentClient, ConsentRequest,
    DependOnIdentitySessionVerifier, DependOnOAuth2FlowGateway, IdentitySessionVerifier,
    LoginChallenge, LoginContext, LoginRequest, OAuth2FlowGateway, RedirectTarget, Rejection,
    UpstreamError, UpstreamErrorKind, VerifiedSession,
};
use std::sync::{Arc, Mutex};

const ACCEPTED: &str = "http://provider/accepted";
const REJECTED: &str = "http://provider/rejected";

#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    GetLogin(String),
    AcceptLogin(String, AcceptLogin),
    RejectLogin(String, Rejection),
    GetConsent(String),
    AcceptConsent(String, AcceptConsent),
    RejectConsent(String, Rejection),
}

#[derive(Clone)]
struct FakeGateway {
    login_request: Arc<Mutex<Result<LoginRequest, UpstreamError>>>,
    consent_request: Arc<Mutex<Result<ConsentRequest, UpstreamError>>>,
    accept_error: Arc<Mutex<Option<UpstreamError>>>,
    reject_error: Arc<Mutex<Option<UpstreamError>>>,
    calls: Arc<Mutex<Vec<Call>>>,
}

fn upstream(kind: UpstreamErrorKind) -> UpstreamError {
    UpstreamError::new(kind, "fake upstream failure")
}

fn login_request(skip: bool) -> LoginRequest {
    LoginRequest {
        skip,
        subject: "provider-subject".to_string(),
    }
}

fn consent_request() -> ConsentRequest {
    ConsentRequest {
        skip: false,
        client: Some(ConsentClient {
            client_name: Some("My App".to_string()),
            skip_consent: Some(false),
        }),
        requested_scope: strings(&["openid", "email", "profile"]),
        requested_access_token_audience: strings(&["account"]),
        context: LoginContext {
            email: Some("a@example.com".to_string()),
        },
    }
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

impl FakeGateway {
    fn new() -> Self {
        Self {
            login_request: Arc::new(Mutex::new(Ok(login_request(false)))),
            consent_request: Arc::new(Mutex::new(Ok(consent_request()))),
            accept_error: Arc::default(),
            reject_error: Arc::default(),
            calls: Arc::default(),
        }
    }

    fn record(&self, call: Call) {
        self.calls.lock().unwrap().push(call);
    }

    fn result(
        &self,
        error: &Mutex<Option<UpstreamError>>,
        url: &str,
    ) -> Result<RedirectTarget, UpstreamError> {
        match error.lock().unwrap().clone() {
            Some(error) => Err(error),
            None => Ok(RedirectTarget::new(url)),
        }
    }
}

impl OAuth2FlowGateway for FakeGateway {
    async fn get_login_request(
        &self,
        challenge: &LoginChallenge,
    ) -> Result<LoginRequest, UpstreamError> {
        self.record(Call::GetLogin(challenge.as_str().to_string()));
        self.login_request.lock().unwrap().clone()
    }

    async fn accept_login(
        &self,
        challenge: &LoginChallenge,
        accept: &AcceptLogin,
    ) -> Result<RedirectTarget, UpstreamError> {
        self.record(Call::AcceptLogin(
            challenge.as_str().to_string(),
            accept.clone(),
        ));
        self.result(&self.accept_error, ACCEPTED)
    }

    async fn reject_login(
        &self,
        challenge: &LoginChallenge,
        rejection: &Rejection,
    ) -> Result<RedirectTarget, UpstreamError> {
        self.record(Call::RejectLogin(
            challenge.as_str().to_string(),
            rejection.clone(),
        ));
        self.result(&self.reject_error, REJECTED)
    }

    async fn get_consent_request(
        &self,
        challenge: &ConsentChallenge,
    ) -> Result<ConsentRequest, UpstreamError> {
        self.record(Call::GetConsent(challenge.as_str().to_string()));
        self.consent_request.lock().unwrap().clone()
    }

    async fn accept_consent(
        &self,
        challenge: &ConsentChallenge,
        accept: &AcceptConsent,
    ) -> Result<RedirectTarget, UpstreamError> {
        self.record(Call::AcceptConsent(
            challenge.as_str().to_string(),
            accept.clone(),
        ));
        self.result(&self.accept_error, ACCEPTED)
    }

    async fn reject_consent(
        &self,
        challenge: &ConsentChallenge,
        rejection: &Rejection,
    ) -> Result<RedirectTarget, UpstreamError> {
        self.record(Call::RejectConsent(
            challenge.as_str().to_string(),
            rejection.clone(),
        ));
        self.result(&self.reject_error, REJECTED)
    }
}

#[derive(Clone)]
struct FakeVerifier {
    result: Arc<Mutex<Result<Option<VerifiedSession>, UpstreamError>>>,
    cookies: Arc<Mutex<Vec<String>>>,
}

impl FakeVerifier {
    fn answering(result: Result<Option<VerifiedSession>, UpstreamError>) -> Self {
        Self {
            result: Arc::new(Mutex::new(result)),
            cookies: Arc::default(),
        }
    }
}

impl IdentitySessionVerifier for FakeVerifier {
    async fn verify_session(
        &self,
        session_cookie: &str,
    ) -> Result<Option<VerifiedSession>, UpstreamError> {
        self.cookies
            .lock()
            .unwrap()
            .push(session_cookie.to_string());
        self.result.lock().unwrap().clone()
    }
}

#[derive(Clone)]
struct Module {
    gateway: FakeGateway,
    verifier: FakeVerifier,
}

impl DependOnOAuth2FlowGateway for Module {
    type OAuth2FlowGateway = FakeGateway;
    fn oauth2_flow_gateway(&self) -> &FakeGateway {
        &self.gateway
    }
}

impl DependOnIdentitySessionVerifier for Module {
    type IdentitySessionVerifier = FakeVerifier;
    fn identity_session_verifier(&self) -> &FakeVerifier {
        &self.verifier
    }
}

fn session(email: Option<&str>) -> VerifiedSession {
    VerifiedSession {
        identity_id: "identity-1".to_string(),
        email: email.map(String::from),
    }
}

fn module_with_session(result: Result<Option<VerifiedSession>, UpstreamError>) -> Module {
    Module {
        gateway: FakeGateway::new(),
        verifier: FakeVerifier::answering(result),
    }
}

fn fixture() -> Module {
    module_with_session(Ok(Some(session(Some("a@example.com")))))
}

fn calls(module: &Module) -> Vec<Call> {
    module.gateway.calls.lock().unwrap().clone()
}

fn verifier_cookies(module: &Module) -> Vec<String> {
    module.verifier.cookies.lock().unwrap().clone()
}

fn login_challenge() -> LoginChallenge {
    LoginChallenge::new("login-1")
}

fn consent_challenge() -> ConsentChallenge {
    ConsentChallenge::new("consent-1")
}

fn login_required() -> Rejection {
    Rejection {
        error: "login_required".to_string(),
        error_description: Some("No valid Kratos session found.".to_string()),
    }
}

fn accept_login_call(subject: &str, email: Option<&str>) -> Call {
    Call::AcceptLogin(
        "login-1".to_string(),
        AcceptLogin {
            subject: subject.to_string(),
            remember: true,
            remember_for_secs: 3600,
            context: email.map(|email| LoginContext {
                email: Some(email.to_string()),
            }),
        },
    )
}

fn accept_consent_call(scopes: &[&str], audience: &[&str], email: Option<&str>) -> Call {
    Call::AcceptConsent(
        "consent-1".to_string(),
        AcceptConsent {
            grant_scope: strings(scopes),
            grant_access_token_audience: strings(audience),
            remember: true,
            remember_for_secs: 3600,
            id_token_email: email.map(String::from),
        },
    )
}

// ---------------------------------------------------------------------------
// login
// ---------------------------------------------------------------------------

#[tokio::test]
async fn login_skip_accepts_the_providers_subject_without_asking_the_identity_provider() {
    let module = fixture();
    *module.gateway.login_request.lock().unwrap() = Ok(login_request(true));

    let redirect = module.login(&login_challenge(), "").await.unwrap();

    assert_eq!(redirect, RedirectTarget::new(ACCEPTED));
    assert_eq!(
        calls(&module),
        vec![
            Call::GetLogin("login-1".to_string()),
            accept_login_call("provider-subject", None),
        ]
    );
    assert!(verifier_cookies(&module).is_empty());
}

#[tokio::test]
async fn login_with_a_valid_session_accepts_the_identity_id_with_the_email_context() {
    let module = fixture();

    let redirect = module
        .login(&login_challenge(), "ory_kratos_session=tok")
        .await
        .unwrap();

    assert_eq!(redirect, RedirectTarget::new(ACCEPTED));
    assert_eq!(
        calls(&module),
        vec![
            Call::GetLogin("login-1".to_string()),
            accept_login_call("identity-1", Some("a@example.com")),
        ]
    );
}

#[tokio::test]
async fn login_with_a_session_without_email_accepts_without_context() {
    let module = module_with_session(Ok(Some(session(None))));

    module
        .login(&login_challenge(), "ory_kratos_session=tok")
        .await
        .unwrap();

    assert_eq!(
        calls(&module).last(),
        Some(&accept_login_call("identity-1", None))
    );
}

#[tokio::test]
async fn login_forwards_only_the_session_cookie() {
    let module = fixture();

    module
        .login(
            &login_challenge(),
            "tracking=abc; ory_kratos_session=tok; theme=dark",
        )
        .await
        .unwrap();

    assert_eq!(verifier_cookies(&module), vec!["ory_kratos_session=tok"]);
}

#[tokio::test]
async fn login_without_cookie_header_rejects_without_asking_the_identity_provider() {
    let module = fixture();

    let redirect = module.login(&login_challenge(), "").await.unwrap();

    assert_eq!(redirect, RedirectTarget::new(REJECTED));
    assert_eq!(
        calls(&module),
        vec![
            Call::GetLogin("login-1".to_string()),
            Call::RejectLogin("login-1".to_string(), login_required()),
        ]
    );
    assert!(verifier_cookies(&module).is_empty());
}

#[tokio::test]
async fn login_without_a_session_cookie_rejects_without_asking_the_identity_provider() {
    let module = fixture();

    let redirect = module
        .login(&login_challenge(), "tracking=abc; theme=dark")
        .await
        .unwrap();

    assert_eq!(redirect, RedirectTarget::new(REJECTED));
    assert!(verifier_cookies(&module).is_empty());
    assert!(matches!(calls(&module).last(), Some(Call::RejectLogin(..))));
}

#[tokio::test]
async fn login_with_an_invalid_or_inactive_session_rejects() {
    let module = module_with_session(Ok(None));

    let redirect = module
        .login(&login_challenge(), "ory_kratos_session=expired")
        .await
        .unwrap();

    assert_eq!(redirect, RedirectTarget::new(REJECTED));
    assert_eq!(
        calls(&module).last(),
        Some(&Call::RejectLogin("login-1".to_string(), login_required()))
    );
}

#[tokio::test]
async fn login_rejects_when_the_identity_provider_fails_in_any_way() {
    for kind in [
        UpstreamErrorKind::Unreachable,
        UpstreamErrorKind::Status(500),
        UpstreamErrorKind::Status(503),
        UpstreamErrorKind::InvalidResponse,
    ] {
        let module = module_with_session(Err(upstream(kind)));

        let redirect = module
            .login(&login_challenge(), "ory_kratos_session=tok")
            .await
            .unwrap();

        assert_eq!(redirect, RedirectTarget::new(REJECTED), "{kind:?}");
        assert_eq!(
            calls(&module).last(),
            Some(&Call::RejectLogin("login-1".to_string(), login_required())),
            "{kind:?}"
        );
    }
}

#[tokio::test]
async fn login_returns_the_providers_error_when_the_request_cannot_be_fetched() {
    for kind in [
        UpstreamErrorKind::Unreachable,
        UpstreamErrorKind::Status(404),
        UpstreamErrorKind::Status(500),
        UpstreamErrorKind::InvalidResponse,
    ] {
        let module = fixture();
        *module.gateway.login_request.lock().unwrap() = Err(upstream(kind));

        let error = module
            .login(&login_challenge(), "ory_kratos_session=tok")
            .await
            .unwrap_err();

        assert_eq!(error, LoginConsentError::Upstream(upstream(kind)));
        assert_eq!(calls(&module), vec![Call::GetLogin("login-1".to_string())]);
        assert!(verifier_cookies(&module).is_empty());
    }
}

#[tokio::test]
async fn login_returns_the_providers_error_when_accepting_fails() {
    for skip in [true, false] {
        let module = fixture();
        *module.gateway.login_request.lock().unwrap() = Ok(login_request(skip));
        *module.gateway.accept_error.lock().unwrap() =
            Some(upstream(UpstreamErrorKind::Status(500)));

        let error = module
            .login(&login_challenge(), "ory_kratos_session=tok")
            .await
            .unwrap_err();

        assert_eq!(
            error,
            LoginConsentError::Upstream(upstream(UpstreamErrorKind::Status(500)))
        );
    }
}

#[tokio::test]
async fn login_returns_the_providers_error_when_rejecting_fails() {
    let module = module_with_session(Ok(None));
    *module.gateway.reject_error.lock().unwrap() = Some(upstream(UpstreamErrorKind::Unreachable));

    let error = module
        .login(&login_challenge(), "ory_kratos_session=tok")
        .await
        .unwrap_err();

    assert_eq!(
        error,
        LoginConsentError::Upstream(upstream(UpstreamErrorKind::Unreachable))
    );
}

// ---------------------------------------------------------------------------
// begin_consent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn consent_skip_accepts_everything_requested_with_the_email_claim() {
    let module = fixture();
    module
        .gateway
        .consent_request
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .skip = true;

    let outcome = module.begin_consent(&consent_challenge()).await.unwrap();

    assert_eq!(
        outcome,
        ConsentOutcome::Redirect(RedirectTarget::new(ACCEPTED))
    );
    assert_eq!(
        calls(&module),
        vec![
            Call::GetConsent("consent-1".to_string()),
            accept_consent_call(
                &["openid", "email", "profile"],
                &["account"],
                Some("a@example.com")
            ),
        ]
    );
}

#[tokio::test]
async fn consent_client_skip_consent_accepts_without_prompting() {
    let module = fixture();
    module
        .gateway
        .consent_request
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .client = Some(ConsentClient {
        client_name: Some("My App".to_string()),
        skip_consent: Some(true),
    });

    let outcome = module.begin_consent(&consent_challenge()).await.unwrap();

    assert_eq!(
        outcome,
        ConsentOutcome::Redirect(RedirectTarget::new(ACCEPTED))
    );
}

#[tokio::test]
async fn consent_without_email_scope_or_email_adds_no_claim() {
    // email scope requested but the login carried no email
    let module = fixture();
    {
        let mut request = module.gateway.consent_request.lock().unwrap();
        let request = request.as_mut().unwrap();
        request.skip = true;
        request.context = LoginContext { email: None };
    }
    module.begin_consent(&consent_challenge()).await.unwrap();
    assert_eq!(
        calls(&module).last(),
        Some(&accept_consent_call(
            &["openid", "email", "profile"],
            &["account"],
            None
        ))
    );

    // the login carried an email but the email scope is not requested
    let module = fixture();
    {
        let mut request = module.gateway.consent_request.lock().unwrap();
        let request = request.as_mut().unwrap();
        request.skip = true;
        request.requested_scope = strings(&["openid"]);
    }
    module.begin_consent(&consent_challenge()).await.unwrap();
    assert_eq!(
        calls(&module).last(),
        Some(&accept_consent_call(&["openid"], &["account"], None))
    );
}

#[tokio::test]
async fn consent_that_needs_the_user_is_returned_as_a_prompt() {
    let module = fixture();

    let outcome = module.begin_consent(&consent_challenge()).await.unwrap();

    assert_eq!(
        outcome,
        ConsentOutcome::Prompt(ConsentPrompt {
            consent_challenge: consent_challenge(),
            client_name: Some("My App".to_string()),
            requested_scope: strings(&["openid", "email", "profile"]),
        })
    );
    assert_eq!(
        calls(&module),
        vec![Call::GetConsent("consent-1".to_string())]
    );
}

#[tokio::test]
async fn consent_prompt_without_client_has_no_client_name() {
    let module = fixture();
    module
        .gateway
        .consent_request
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .client = None;

    let outcome = module.begin_consent(&consent_challenge()).await.unwrap();

    let ConsentOutcome::Prompt(prompt) = outcome else {
        panic!("expected a prompt");
    };
    assert_eq!(prompt.client_name, None);
}

#[tokio::test]
async fn consent_returns_the_providers_error_when_fetching_or_accepting_fails() {
    for kind in [
        UpstreamErrorKind::Unreachable,
        UpstreamErrorKind::Status(404),
        UpstreamErrorKind::Status(500),
        UpstreamErrorKind::InvalidResponse,
    ] {
        let module = fixture();
        *module.gateway.consent_request.lock().unwrap() = Err(upstream(kind));
        let error = module
            .begin_consent(&consent_challenge())
            .await
            .unwrap_err();
        assert_eq!(error, LoginConsentError::Upstream(upstream(kind)));
    }

    let module = fixture();
    module
        .gateway
        .consent_request
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .skip = true;
    *module.gateway.accept_error.lock().unwrap() = Some(upstream(UpstreamErrorKind::Status(409)));
    let error = module
        .begin_consent(&consent_challenge())
        .await
        .unwrap_err();
    assert_eq!(
        error,
        LoginConsentError::Upstream(upstream(UpstreamErrorKind::Status(409)))
    );
}

// ---------------------------------------------------------------------------
// decide_consent
// ---------------------------------------------------------------------------

fn accept_decision(scopes: Option<&[&str]>) -> ConsentDecision {
    ConsentDecision {
        challenge: consent_challenge(),
        accept: true,
        grant_scope: scopes.map(strings),
    }
}

#[tokio::test]
async fn accepted_consent_grants_the_subset_with_the_requested_audience() {
    let module = fixture();

    let redirect = module
        .decide_consent(accept_decision(Some(&["openid", "email"])))
        .await
        .unwrap();

    assert_eq!(redirect, RedirectTarget::new(ACCEPTED));
    assert_eq!(
        calls(&module),
        vec![
            Call::GetConsent("consent-1".to_string()),
            accept_consent_call(&["openid", "email"], &["account"], Some("a@example.com")),
        ]
    );
}

#[tokio::test]
async fn email_claim_follows_the_granted_scope_not_the_requested_one() {
    let module = fixture();

    module
        .decide_consent(accept_decision(Some(&["openid", "profile"])))
        .await
        .unwrap();

    assert_eq!(
        calls(&module).last(),
        Some(&accept_consent_call(
            &["openid", "profile"],
            &["account"],
            None
        ))
    );
}

#[tokio::test]
async fn accepted_consent_without_scopes_grants_nothing() {
    let module = fixture();

    module.decide_consent(accept_decision(None)).await.unwrap();

    assert_eq!(
        calls(&module).last(),
        Some(&accept_consent_call(&[], &["account"], None))
    );
}

#[tokio::test]
async fn granting_an_unrequested_scope_is_refused_before_accepting() {
    let module = fixture();

    let error = module
        .decide_consent(accept_decision(Some(&["openid", "admin"])))
        .await
        .unwrap_err();

    assert_eq!(
        error,
        LoginConsentError::ScopeNotRequested("admin".to_string())
    );
    assert_eq!(
        calls(&module),
        vec![Call::GetConsent("consent-1".to_string())]
    );
}

#[tokio::test]
async fn denied_consent_rejects_without_fetching_the_request() {
    let module = fixture();

    let redirect = module
        .decide_consent(ConsentDecision {
            challenge: consent_challenge(),
            accept: false,
            grant_scope: Some(strings(&["openid"])),
        })
        .await
        .unwrap();

    assert_eq!(redirect, RedirectTarget::new(REJECTED));
    assert_eq!(
        calls(&module),
        vec![Call::RejectConsent(
            "consent-1".to_string(),
            Rejection {
                error: "consent_denied".to_string(),
                error_description: Some("The user denied the consent request.".to_string()),
            }
        )]
    );
}

#[tokio::test]
async fn decision_returns_the_providers_error_at_every_step() {
    let module = fixture();
    *module.gateway.consent_request.lock().unwrap() = Err(upstream(UpstreamErrorKind::Status(404)));
    let error = module
        .decide_consent(accept_decision(Some(&["openid"])))
        .await
        .unwrap_err();
    assert_eq!(
        error,
        LoginConsentError::Upstream(upstream(UpstreamErrorKind::Status(404)))
    );

    let module = fixture();
    *module.gateway.accept_error.lock().unwrap() = Some(upstream(UpstreamErrorKind::Status(500)));
    let error = module
        .decide_consent(accept_decision(Some(&["openid"])))
        .await
        .unwrap_err();
    assert_eq!(
        error,
        LoginConsentError::Upstream(upstream(UpstreamErrorKind::Status(500)))
    );

    let module = fixture();
    *module.gateway.reject_error.lock().unwrap() = Some(upstream(UpstreamErrorKind::Unreachable));
    let error = module
        .decide_consent(ConsentDecision {
            challenge: consent_challenge(),
            accept: false,
            grant_scope: None,
        })
        .await
        .unwrap_err();
    assert_eq!(
        error,
        LoginConsentError::Upstream(upstream(UpstreamErrorKind::Unreachable))
    );
}
