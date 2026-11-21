use super::{LoginConsentError, REMEMBER_FOR_SECS};
use kernel::interfaces::login_consent::{
    AcceptLogin, DependOnIdentitySessionVerifier, DependOnOAuth2FlowGateway,
    IdentitySessionVerifier, LoginChallenge, LoginContext, OAuth2FlowGateway, RedirectTarget,
    Rejection, VerifiedSession,
};
use std::future::Future;

const SESSION_COOKIE_PREFIX: &str = "ory_kratos_session=";

/// The identity session cookie of a raw `Cookie` header, so that no other cookie is ever forwarded.
fn session_cookie(cookie_header: &str) -> Option<&str> {
    cookie_header
        .split(';')
        .map(|cookie| cookie.trim())
        .find(|cookie| cookie.starts_with(SESSION_COOKIE_PREFIX))
}

/// The verified session behind the request, or `None` for every way of not having one: no cookie
/// at all, no identity session cookie, a session the provider does not accept, and a provider that
/// cannot answer (the last one is logged as an error, not treated differently).
async fn verified_session<V: IdentitySessionVerifier>(
    verifier: &V,
    cookie_header: &str,
) -> Option<VerifiedSession> {
    if cookie_header.is_empty() {
        tracing::warn!("oauth2/login: no cookie header, cannot verify identity session");
        return None;
    }
    let Some(cookie) = session_cookie(cookie_header) else {
        tracing::warn!("oauth2/login: no ory_kratos_session cookie found");
        return None;
    };
    match verifier.verify_session(cookie).await {
        Ok(Some(session)) => Some(session),
        Ok(None) => {
            tracing::warn!("oauth2/login: identity session invalid or expired");
            None
        }
        Err(e) => {
            tracing::error!("Identity session verification request failed: {e}");
            None
        }
    }
}

pub trait LoginUseCase:
    'static + Sync + Send + DependOnOAuth2FlowGateway + DependOnIdentitySessionVerifier
{
    /// Completes the provider's login challenge.
    ///
    /// * A challenge the provider already authenticated (`skip`) is accepted as is.
    /// * Otherwise the identity session cookie picked out of `cookie_header` (the raw `Cookie`
    ///   header, empty when absent; the other cookies are dropped here) decides. A verified session
    ///   is accepted with its identity id as the subject and its email as the login context. Any
    ///   other outcome, including an identity provider that is down, rejects the login with
    ///   `login_required`.
    ///
    /// Failures of the OAuth2 provider itself are returned as errors.
    fn login<'a>(
        &'a self,
        challenge: &'a LoginChallenge,
        cookie_header: &'a str,
    ) -> impl Future<Output = Result<RedirectTarget, LoginConsentError>> + Send + 'a {
        async move {
            let gateway = self.oauth2_flow_gateway();

            let request = gateway.get_login_request(challenge).await.map_err(|e| {
                tracing::error!("Failed to get login request from the OAuth2 provider: {e}");
                LoginConsentError::Upstream(e)
            })?;

            if request.skip {
                return gateway
                    .accept_login(
                        challenge,
                        &AcceptLogin {
                            subject: request.subject,
                            remember: true,
                            remember_for_secs: REMEMBER_FOR_SECS,
                            context: None,
                        },
                    )
                    .await
                    .map_err(|e| {
                        tracing::error!("Failed to accept login at the OAuth2 provider: {e}");
                        LoginConsentError::Upstream(e)
                    });
            }

            let Some(session) =
                verified_session(self.identity_session_verifier(), cookie_header).await
            else {
                return gateway
                    .reject_login(
                        challenge,
                        &Rejection {
                            error: "login_required".to_string(),
                            error_description: Some("No valid Kratos session found.".to_string()),
                        },
                    )
                    .await
                    .map_err(|e| {
                        tracing::error!("Failed to reject login at the OAuth2 provider: {e}");
                        LoginConsentError::Upstream(e)
                    });
            };

            gateway
                .accept_login(
                    challenge,
                    &AcceptLogin {
                        subject: session.identity_id,
                        remember: true,
                        remember_for_secs: REMEMBER_FOR_SECS,
                        context: session
                            .email
                            .map(|email| LoginContext { email: Some(email) }),
                    },
                )
                .await
                .map_err(|e| {
                    tracing::error!("Failed to accept login at the OAuth2 provider: {e}");
                    LoginConsentError::Upstream(e)
                })
        }
    }
}

impl<T> LoginUseCase for T where
    T: 'static + Sync + Send + DependOnOAuth2FlowGateway + DependOnIdentitySessionVerifier
{
}

#[cfg(test)]
mod tests {
    use super::session_cookie;

    #[test]
    fn picks_only_the_session_cookie() {
        assert_eq!(
            session_cookie("a=1; ory_kratos_session=tok; b=2"),
            Some("ory_kratos_session=tok")
        );
        assert_eq!(
            session_cookie("  ory_kratos_session=tok  "),
            Some("ory_kratos_session=tok")
        );
    }

    #[test]
    fn other_cookies_never_match() {
        assert_eq!(session_cookie("a=1; b=2"), None);
        assert_eq!(session_cookie("xory_kratos_session=tok"), None);
        assert_eq!(session_cookie("ory_kratos_session_other=tok"), None);
        assert_eq!(session_cookie(";;"), None);
    }

    #[test]
    fn first_session_cookie_wins() {
        assert_eq!(
            session_cookie("ory_kratos_session=one; ory_kratos_session=two"),
            Some("ory_kratos_session=one")
        );
    }
}
