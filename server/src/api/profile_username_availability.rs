use crate::handler::AppModule;
use application::service::profile_username_availability::{
    CheckProfileUsernameAvailabilityUseCase, ProfileUsernameAvailability,
};
use axum::extract::FromRef;
use kernel::KernelError;
use std::sync::Arc;

#[derive(Clone)]
pub struct ProfileUsernameAvailabilityApi {
    module: Arc<AppModule>,
}

impl ProfileUsernameAvailabilityApi {
    pub fn new(module: Arc<AppModule>) -> Self {
        Self { module }
    }

    pub async fn check(
        &self,
        username: &str,
    ) -> error_stack::Result<ProfileUsernameAvailability, KernelError> {
        self.module
            .check_profile_username_availability(username)
            .await
    }
}

impl FromRef<AppModule> for ProfileUsernameAvailabilityApi {
    fn from_ref(module: &AppModule) -> Self {
        Self::new(Arc::new(module.clone()))
    }
}
