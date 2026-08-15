use crate::handler::AppModule;
use crate::hydra::HydraAdminClient;
use crate::kratos::KratosClient;
use axum::extract::FromRef;
use std::sync::Arc;

#[derive(Clone)]
pub struct OAuth2Api {
    module: Arc<AppModule>,
}

impl OAuth2Api {
    pub fn new(module: Arc<AppModule>) -> Self {
        Self { module }
    }

    pub fn hydra_admin_client(&self) -> &HydraAdminClient {
        self.module.hydra_admin_client()
    }

    pub fn kratos_client(&self) -> &KratosClient {
        self.module.kratos_client()
    }
}

impl FromRef<AppModule> for OAuth2Api {
    fn from_ref(module: &AppModule) -> Self {
        Self::new(Arc::new(module.clone()))
    }
}
