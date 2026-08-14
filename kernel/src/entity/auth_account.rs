mod client_id;
mod id;

pub use self::client_id::*;
pub use self::id::*;
use crate::entity::AuthHostId;
use destructure::Destructure;
use serde::Deserialize;
use serde::Serialize;
use vodca::{Newln, References};

#[derive(
    Debug, Clone, Hash, Eq, PartialEq, References, Newln, Serialize, Deserialize, Destructure,
)]
pub struct AuthAccount {
    id: AuthAccountId,
    host: AuthHostId,
    client_id: AuthAccountClientId,
}

#[cfg(test)]
mod test {
    use crate::entity::{AuthAccount, AuthAccountClientId, AuthAccountId, AuthHostId};

    #[test]
    fn create_auth_account() {
        crate::ensure_generator_initialized();
        let id = AuthAccountId::default();
        let host = AuthHostId::default();
        let client_id = AuthAccountClientId::new("test-client-id");
        let account = AuthAccount::new(id.clone(), host.clone(), client_id.clone());
        assert_eq!(account.id(), &id);
        assert_eq!(account.host(), &host);
        assert_eq!(account.client_id(), &client_id);
    }
}
