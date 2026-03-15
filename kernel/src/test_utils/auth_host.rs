use crate::entity::{AuthHost, AuthHostId, AuthHostUrl};

use super::unique_auth_host_url;

pub struct AuthHostBuilder {
    id: Option<AuthHostId>,
    url: Option<AuthHostUrl>,
}

impl Default for AuthHostBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthHostBuilder {
    pub fn new() -> Self {
        Self {
            id: None,
            url: None,
        }
    }

    pub fn id(mut self, id: AuthHostId) -> Self {
        self.id = Some(id);
        self
    }

    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(AuthHostUrl::new(url));
        self
    }

    pub fn build(self) -> AuthHost {
        crate::ensure_generator_initialized();
        AuthHost::new(
            self.id.unwrap_or_default(),
            self.url.unwrap_or_else(unique_auth_host_url),
        )
    }
}
