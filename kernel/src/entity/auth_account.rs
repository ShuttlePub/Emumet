mod client_id;
mod id;

pub use self::client_id::*;
pub use self::id::*;
use crate::entity::{
    AuthHostId, CommandEnvelope, EventEnvelope, EventId, EventVersion, KnownEventVersion,
};
use crate::event::EventApplier;
use crate::KernelError;
use destructure::Destructure;
use error_stack::Report;
use serde::Deserialize;
use serde::Serialize;
use vodca::{Nameln, Newln, References};

#[derive(
    Debug, Clone, Hash, Eq, PartialEq, References, Newln, Serialize, Deserialize, Destructure,
)]
pub struct AuthAccount {
    id: AuthAccountId,
    host: AuthHostId,
    client_id: AuthAccountClientId,
    version: EventVersion<AuthAccount>,
}

#[derive(Debug, Clone, Eq, PartialEq, Nameln, Serialize, Deserialize)]
#[serde(tag = "type", rename_all_fields = "snake_case")]
#[vodca(prefix = "auth_account", snake_case)]
pub enum AuthAccountEvent {
    Created {
        host: AuthHostId,
        client_id: AuthAccountClientId,
    },
    Deleted,
}

impl AuthAccount {
    pub fn create(
        id: AuthAccountId,
        host: AuthHostId,
        client_id: AuthAccountClientId,
    ) -> CommandEnvelope<AuthAccountEvent, AuthAccount> {
        let event = AuthAccountEvent::Created { host, client_id };
        CommandEnvelope::new(
            EventId::from(id),
            event.name(),
            event,
            Some(KnownEventVersion::Nothing),
        )
    }

    pub fn delete(
        id: AuthAccountId,
        current_version: EventVersion<AuthAccount>,
    ) -> CommandEnvelope<AuthAccountEvent, AuthAccount> {
        let event = AuthAccountEvent::Deleted;
        CommandEnvelope::new(
            EventId::from(id),
            event.name(),
            event,
            Some(KnownEventVersion::Prev(current_version)),
        )
    }
}

impl EventApplier for AuthAccount {
    type Event = AuthAccountEvent;
    const ENTITY_NAME: &'static str = "AuthAccount";

    fn apply(
        entity: &mut Option<Self>,
        event: EventEnvelope<Self::Event, Self>,
    ) -> error_stack::Result<(), KernelError>
    where
        Self: Sized,
    {
        match event.event {
            AuthAccountEvent::Created { host, client_id } => {
                if let Some(entity) = entity {
                    return Err(Report::new(KernelError::Internal)
                        .attach_printable(Self::already_exists(entity)));
                }
                *entity = Some(AuthAccount {
                    id: AuthAccountId::new(event.id),
                    host,
                    client_id,
                    version: event.version,
                });
            }
            AuthAccountEvent::Deleted => {
                if entity.is_none() {
                    return Err(Report::new(KernelError::Internal)
                        .attach_printable(Self::not_exists(event.id.as_ref())));
                }
                *entity = None;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use crate::entity::{
        AuthAccount, AuthAccountClientId, AuthAccountId, AuthHostId, EventEnvelope, EventVersion,
    };
    use crate::event::EventApplier;
    use uuid::Uuid;

    #[test]
    fn create_auth_account() {
        let id = AuthAccountId::new(Uuid::now_v7());
        let host = AuthHostId::new(Uuid::now_v7());
        let client_id = AuthAccountClientId::new(Uuid::now_v7());
        let create_account = AuthAccount::create(id.clone(), host.clone(), client_id.clone());
        let envelope = EventEnvelope::new(
            create_account.id().clone(),
            create_account.event().clone(),
            EventVersion::new(Uuid::now_v7()),
        );
        let mut account = None;
        AuthAccount::apply(&mut account, envelope).unwrap();
        assert!(account.is_some());
        let account = account.unwrap();
        assert_eq!(account.id(), &id);
        assert_eq!(account.host(), &host);
        assert_eq!(account.client_id(), &client_id);
    }

    #[test]
    fn delete_auth_account() {
        let id = AuthAccountId::new(Uuid::now_v7());
        let host = AuthHostId::new(Uuid::now_v7());
        let client_id = AuthAccountClientId::new(Uuid::now_v7());
        let account = AuthAccount::new(
            id.clone(),
            host.clone(),
            client_id.clone(),
            EventVersion::new(Uuid::now_v7()),
        );
        let delete_account = AuthAccount::delete(id.clone(), account.version().clone());
        let envelope = EventEnvelope::new(
            delete_account.id().clone(),
            delete_account.event().clone(),
            EventVersion::new(Uuid::now_v7()),
        );
        let mut account = Some(account);
        AuthAccount::apply(&mut account, envelope).unwrap();
        assert!(account.is_none());
    }
}
