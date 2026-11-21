use super::{LoginConsentError, REMEMBER_FOR_SECS};
use kernel::interfaces::login_consent::{
    AcceptConsent, ConsentChallenge, DependOnOAuth2FlowGateway, LoginContext, OAuth2FlowGateway,
    RedirectTarget, Rejection,
};
use std::collections::HashSet;
use std::future::Future;

/// What the user is shown when the consent cannot be decided automatically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsentPrompt {
    pub consent_challenge: ConsentChallenge,
    pub client_name: Option<String>,
    pub requested_scope: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsentOutcome {
    /// The consent was settled without asking the user.
    Redirect(RedirectTarget),
    Prompt(ConsentPrompt),
}

/// The user's answer to a consent prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsentDecision {
    pub challenge: ConsentChallenge,
    pub accept: bool,
    pub grant_scope: Option<Vec<String>>,
}

/// The ID token's `email` claim: present only when the `email` scope is granted and the login
/// carried an email.
fn id_token_email(context: &LoginContext, granted_scopes: &[String]) -> Option<String> {
    if granted_scopes.iter().any(|scope| scope == "email") {
        context.email.clone()
    } else {
        None
    }
}

fn upstream(
    action: &str,
    e: kernel::interfaces::login_consent::UpstreamError,
) -> LoginConsentError {
    tracing::error!("Failed to {action} at the OAuth2 provider: {e}");
    LoginConsentError::Upstream(e)
}

pub trait ConsentUseCase: 'static + Sync + Send + DependOnOAuth2FlowGateway {
    /// Looks at the consent request. A request the provider marks `skip`, or whose client is
    /// configured with `skip_consent`, is accepted with everything that was requested; any other is
    /// returned as a prompt.
    fn begin_consent<'a>(
        &'a self,
        challenge: &'a ConsentChallenge,
    ) -> impl Future<Output = Result<ConsentOutcome, LoginConsentError>> + Send + 'a {
        async move {
            let gateway = self.oauth2_flow_gateway();

            let request = gateway.get_consent_request(challenge).await.map_err(|e| {
                tracing::error!("Failed to get consent request from the OAuth2 provider: {e}");
                LoginConsentError::Upstream(e)
            })?;

            let skip_consent = request
                .client
                .as_ref()
                .and_then(|client| client.skip_consent)
                .unwrap_or(false);

            if request.skip || skip_consent {
                let id_token_email = id_token_email(&request.context, &request.requested_scope);
                let redirect = gateway
                    .accept_consent(
                        challenge,
                        &AcceptConsent {
                            grant_scope: request.requested_scope,
                            grant_access_token_audience: request.requested_access_token_audience,
                            remember: true,
                            remember_for_secs: REMEMBER_FOR_SECS,
                            id_token_email,
                        },
                    )
                    .await
                    .map_err(|e| upstream("accept consent", e))?;
                return Ok(ConsentOutcome::Redirect(redirect));
            }

            Ok(ConsentOutcome::Prompt(ConsentPrompt {
                consent_challenge: challenge.clone(),
                client_name: request.client.and_then(|client| client.client_name),
                requested_scope: request.requested_scope,
            }))
        }
    }

    /// Applies the user's decision. An acceptance is checked against the provider's own record of
    /// the request: every granted scope must have been requested, and the audience is the requested
    /// one (never taken from the caller).
    fn decide_consent<'a>(
        &'a self,
        decision: ConsentDecision,
    ) -> impl Future<Output = Result<RedirectTarget, LoginConsentError>> + Send + 'a {
        async move {
            let gateway = self.oauth2_flow_gateway();
            let ConsentDecision {
                challenge,
                accept,
                grant_scope,
            } = decision;

            if !accept {
                return gateway
                    .reject_consent(
                        &challenge,
                        &Rejection {
                            error: "consent_denied".to_string(),
                            error_description: Some(
                                "The user denied the consent request.".to_string(),
                            ),
                        },
                    )
                    .await
                    .map_err(|e| upstream("reject consent", e));
            }

            let grant_scope = grant_scope.unwrap_or_default();

            let request = gateway.get_consent_request(&challenge).await.map_err(|e| {
                tracing::error!("Failed to get consent request from the OAuth2 provider: {e}");
                LoginConsentError::Upstream(e)
            })?;

            let requested: HashSet<&str> = request
                .requested_scope
                .iter()
                .map(|scope| scope.as_str())
                .collect();
            for scope in &grant_scope {
                if !requested.contains(scope.as_str()) {
                    tracing::warn!("Client attempted to grant unrequested scope: {scope}");
                    return Err(LoginConsentError::ScopeNotRequested(scope.clone()));
                }
            }

            let id_token_email = id_token_email(&request.context, &grant_scope);
            gateway
                .accept_consent(
                    &challenge,
                    &AcceptConsent {
                        grant_scope,
                        grant_access_token_audience: request.requested_access_token_audience,
                        remember: true,
                        remember_for_secs: REMEMBER_FOR_SECS,
                        id_token_email,
                    },
                )
                .await
                .map_err(|e| upstream("accept consent", e))
        }
    }
}

impl<T> ConsentUseCase for T where T: 'static + Sync + Send + DependOnOAuth2FlowGateway {}
