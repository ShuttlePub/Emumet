mod access_token;
mod client_id;
mod id;
mod refresh_token;

pub use self::access_token::*;
pub use self::client_id::*;
pub use self::id::*;
pub use self::refresh_token::*;
use crate::entity::{CommandEnvelope, EventEnvelope, EventId, KnownEventVersion, StellarHostId};
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
pub struct StellarAccount {
    id: StellarAccountId,
    host: StellarHostId,
    client_id: StellarAccountClientId,
    access_token: StellarAccountAccessToken,
    refresh_token: StellarAccountRefreshToken,
}

#[derive(Debug, Clone, Eq, PartialEq, Nameln, Serialize, Deserialize)]
#[serde(tag = "type", rename_all_fields = "snake_case")]
#[vodca(prefix = "stellar_account", snake_case)]
pub enum StellarAccountEvent {
    Created {
        host: StellarHostId,
        client_id: StellarAccountClientId,
        access_token: StellarAccountAccessToken,
        refresh_token: StellarAccountRefreshToken,
    },
    Updated {
        access_token: StellarAccountAccessToken,
        refresh_token: StellarAccountRefreshToken,
    },
    Deleted,
}

impl StellarAccount {
    pub fn create(
        id: StellarAccountId,
        host: StellarHostId,
        client_id: StellarAccountClientId,
        access_token: StellarAccountAccessToken,
        refresh_token: StellarAccountRefreshToken,
    ) -> CommandEnvelope<StellarAccountEvent, StellarAccount> {
        let event = StellarAccountEvent::Created {
            host,
            client_id,
            access_token,
            refresh_token,
        };
        CommandEnvelope::new(
            EventId::from(id),
            event.name(),
            event,
            Some(KnownEventVersion::Nothing),
        )
    }

    pub fn update(
        id: StellarAccountId,
        access_token: StellarAccountAccessToken,
        refresh_token: StellarAccountRefreshToken,
    ) -> CommandEnvelope<StellarAccountEvent, StellarAccount> {
        let event = StellarAccountEvent::Updated {
            access_token,
            refresh_token,
        };
        CommandEnvelope::new(EventId::from(id), event.name(), event, None)
    }

    pub fn delete(id: StellarAccountId) -> CommandEnvelope<StellarAccountEvent, StellarAccount> {
        let event = StellarAccountEvent::Deleted;
        CommandEnvelope::new(EventId::from(id), event.name(), event, None)
    }
}

impl EventApplier for StellarAccount {
    type Event = StellarAccountEvent;
    const ENTITY_NAME: &'static str = "StellarAccount";

    fn apply(
        entity: &mut Option<Self>,
        event: EventEnvelope<Self::Event, Self>,
    ) -> error_stack::Result<(), KernelError>
    where
        Self: Sized,
    {
        match event.event {
            StellarAccountEvent::Created {
                host,
                client_id,
                access_token,
                refresh_token,
            } => {
                if let Some(entity) = entity {
                    return Err(Report::new(KernelError::Internal)
                        .attach_printable(Self::already_exists(entity)));
                }
                *entity = Some(StellarAccount {
                    id: StellarAccountId::new(event.id),
                    host,
                    client_id,
                    access_token,
                    refresh_token,
                });
            }
            StellarAccountEvent::Updated {
                access_token,
                refresh_token,
            } => {
                if let Some(entity) = entity {
                    entity.access_token = access_token;
                    entity.refresh_token = refresh_token;
                } else {
                    return Err(Report::new(KernelError::Internal)
                        .attach_printable(Self::not_exists(event.id.as_ref())));
                }
            }
            StellarAccountEvent::Deleted => {
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
        CreatedAt, EventEnvelope, EventVersion, StellarAccount, StellarAccountAccessToken,
        StellarAccountClientId, StellarAccountId, StellarAccountRefreshToken, StellarHostId,
    };
    use crate::event::EventApplier;
    use uuid::Uuid;

    #[test]
    fn create_stellar_account() {
        let id = StellarAccountId::new(Uuid::now_v7());
        let host = StellarHostId::new(Uuid::now_v7());
        let client_id = StellarAccountClientId::new(Uuid::now_v7());
        let access_token = StellarAccountAccessToken::new(Uuid::now_v7());
        let refresh_token = StellarAccountRefreshToken::new(Uuid::now_v7());
        let create_account = StellarAccount::create(
            id.clone(),
            host.clone(),
            client_id.clone(),
            access_token.clone(),
            refresh_token.clone(),
        );
        let envelope = EventEnvelope::new(
            create_account.id().clone().into(),
            create_account.event().clone(),
            EventVersion::new(Uuid::now_v7()),
            CreatedAt::now(),
        );
        let mut account = None;
        StellarAccount::apply(&mut account, envelope).unwrap();
        assert!(account.is_some());
        let account = account.unwrap();
        assert_eq!(account.id(), &id);
        assert_eq!(account.host(), &host);
        assert_eq!(account.client_id(), &client_id);
        assert_eq!(account.access_token(), &access_token);
        assert_eq!(account.refresh_token(), &refresh_token);
    }

    #[test]
    fn update_stellar_account() {
        let id = StellarAccountId::new(Uuid::now_v7());
        let host = StellarHostId::new(Uuid::now_v7());
        let client_id = StellarAccountClientId::new(Uuid::now_v7());
        let access_token = StellarAccountAccessToken::new(Uuid::now_v7());
        let refresh_token = StellarAccountRefreshToken::new(Uuid::now_v7());
        let account = StellarAccount::new(
            id.clone(),
            host.clone(),
            client_id.clone(),
            access_token.clone(),
            refresh_token.clone(),
        );
        let new_access_token = StellarAccountAccessToken::new(Uuid::now_v7());
        let new_refresh_token = StellarAccountRefreshToken::new(Uuid::now_v7());
        let update_account = StellarAccount::update(
            id.clone(),
            new_access_token.clone(),
            new_refresh_token.clone(),
        );
        let envelope = EventEnvelope::new(
            update_account.id().clone().into(),
            update_account.event().clone(),
            EventVersion::new(Uuid::now_v7()),
            CreatedAt::now(),
        );
        let mut account = Some(account);
        StellarAccount::apply(&mut account, envelope).unwrap();
        assert!(account.is_some());
        let account = account.unwrap();
        assert_eq!(account.id(), &id);
        assert_eq!(account.host(), &host);
        assert_eq!(account.client_id(), &client_id);
        assert_eq!(account.access_token(), &new_access_token);
        assert_eq!(account.refresh_token(), &new_refresh_token);
    }

    #[test]
    fn delete_stellar_account() {
        let id = StellarAccountId::new(Uuid::now_v7());
        let host = StellarHostId::new(Uuid::now_v7());
        let client_id = StellarAccountClientId::new(Uuid::now_v7());
        let access_token = StellarAccountAccessToken::new(Uuid::now_v7());
        let refresh_token = StellarAccountRefreshToken::new(Uuid::now_v7());
        let account = StellarAccount::new(
            id.clone(),
            host.clone(),
            client_id.clone(),
            access_token.clone(),
            refresh_token.clone(),
        );
        let delete_account = StellarAccount::delete(id.clone());
        let envelope = EventEnvelope::new(
            delete_account.id().clone().into(),
            delete_account.event().clone(),
            EventVersion::new(Uuid::now_v7()),
            CreatedAt::now(),
        );
        let mut account = Some(account);
        StellarAccount::apply(&mut account, envelope).unwrap();
        assert!(account.is_none());
    }
}
