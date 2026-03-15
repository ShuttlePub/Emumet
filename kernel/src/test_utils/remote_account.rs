use crate::entity::{ImageId, RemoteAccount, RemoteAccountAcct, RemoteAccountId, RemoteAccountUrl};

use super::unique_remote_acct;

pub struct RemoteAccountBuilder {
    id: Option<RemoteAccountId>,
    acct: Option<RemoteAccountAcct>,
    url: Option<RemoteAccountUrl>,
    icon_id: Option<Option<ImageId>>,
}

impl Default for RemoteAccountBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl RemoteAccountBuilder {
    pub fn new() -> Self {
        Self {
            id: None,
            acct: None,
            url: None,
            icon_id: None,
        }
    }

    pub fn id(mut self, id: RemoteAccountId) -> Self {
        self.id = Some(id);
        self
    }

    pub fn acct(mut self, acct: impl Into<String>) -> Self {
        self.acct = Some(RemoteAccountAcct::new(acct));
        self
    }

    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(RemoteAccountUrl::new(url));
        self
    }

    pub fn icon_id(mut self, icon_id: Option<ImageId>) -> Self {
        self.icon_id = Some(icon_id);
        self
    }

    pub fn build(self) -> RemoteAccount {
        crate::ensure_generator_initialized();
        let (default_acct, default_url) = unique_remote_acct();
        RemoteAccount::new(
            self.id
                .unwrap_or_else(|| RemoteAccountId::new(crate::generate_id())),
            self.acct.unwrap_or(default_acct),
            self.url.unwrap_or(default_url),
            self.icon_id.unwrap_or(None),
        )
    }
}
