use crate::entity::{AuthAccount, AuthAccountClientId, AuthAccountId, AuthHostId, EventVersion};

use super::DEFAULT_CLIENT_ID;

pub struct AuthAccountBuilder {
    id: Option<AuthAccountId>,
    host: Option<AuthHostId>,
    client_id: Option<AuthAccountClientId>,
    version: Option<EventVersion<AuthAccount>>,
}

impl Default for AuthAccountBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthAccountBuilder {
    pub fn new() -> Self {
        Self {
            id: None,
            host: None,
            client_id: None,
            version: None,
        }
    }

    pub fn id(mut self, id: AuthAccountId) -> Self {
        self.id = Some(id);
        self
    }

    pub fn host(mut self, host: AuthHostId) -> Self {
        self.host = Some(host);
        self
    }

    pub fn client_id(mut self, client_id: impl Into<String>) -> Self {
        self.client_id = Some(AuthAccountClientId::new(client_id));
        self
    }

    pub fn version(mut self, version: EventVersion<AuthAccount>) -> Self {
        self.version = Some(version);
        self
    }

    pub fn build(self) -> AuthAccount {
        crate::ensure_generator_initialized();
        AuthAccount::new(
            self.id.unwrap_or_default(),
            self.host.unwrap_or_default(),
            self.client_id
                .unwrap_or_else(|| AuthAccountClientId::new(DEFAULT_CLIENT_ID)),
            self.version.unwrap_or_default(),
        )
    }
}
