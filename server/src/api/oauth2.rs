use crate::handler::AppModule;
use application::service::login_consent::{
    ConsentDecision, ConsentOutcome, ConsentUseCase, LoginConsentError, LoginUseCase,
};
use axum::extract::FromRef;
use kernel::interfaces::login_consent::{ConsentChallenge, LoginChallenge, RedirectTarget};
use std::sync::Arc;

#[derive(Clone)]
pub struct OAuth2Api {
    module: Arc<AppModule>,
}

impl OAuth2Api {
    pub fn new(module: Arc<AppModule>) -> Self {
        Self { module }
    }

    pub async fn login(
        &self,
        challenge: &LoginChallenge,
        cookie_header: &str,
    ) -> Result<RedirectTarget, LoginConsentError> {
        self.module.login(challenge, cookie_header).await
    }

    pub async fn begin_consent(
        &self,
        challenge: &ConsentChallenge,
    ) -> Result<ConsentOutcome, LoginConsentError> {
        self.module.begin_consent(challenge).await
    }

    pub async fn decide_consent(
        &self,
        decision: ConsentDecision,
    ) -> Result<RedirectTarget, LoginConsentError> {
        self.module.decide_consent(decision).await
    }
}

impl FromRef<AppModule> for OAuth2Api {
    fn from_ref(module: &AppModule) -> Self {
        Self::new(Arc::new(module.clone()))
    }
}
