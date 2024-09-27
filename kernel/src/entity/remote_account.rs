mod acct;
mod id;
mod url;

pub use self::acct::*;
pub use self::id::*;
pub use self::url::*;
use crate::entity::image::ImageId;
use crate::entity::{CommandEnvelope, EventEnvelope, EventId, KnownEventVersion};
use crate::event::EventApplier;
use crate::KernelError;
use error_stack::Report;
use serde::{Deserialize, Serialize};
use vodca::{Nameln, Newln, References};

#[derive(Debug, Clone, Eq, PartialEq, References, Newln, Serialize, Deserialize)]
pub struct RemoteAccount {
    id: RemoteAccountId,
    acct: RemoteAccountAcct,
    url: RemoteAccountUrl,
    icon_id: Option<ImageId>,
}

#[derive(Debug, Clone, Eq, PartialEq, Nameln, Serialize, Deserialize)]
#[serde(tag = "type", rename_all_fields = "snake_case")]
#[vodca(prefix = "remote_account", snake_case)]
pub enum RemoteAccountEvent {
    Created {
        acct: RemoteAccountAcct,
        url: RemoteAccountUrl,
        icon_id: Option<ImageId>,
    },
    Updated {
        icon_id: Option<ImageId>,
    },
    Deleted,
}

impl RemoteAccount {
    pub fn create(
        id: RemoteAccountId,
        acct: RemoteAccountAcct,
        url: RemoteAccountUrl,
        icon_id: Option<ImageId>,
    ) -> CommandEnvelope<RemoteAccountEvent, RemoteAccount> {
        let event = RemoteAccountEvent::Created { acct, url, icon_id };
        CommandEnvelope::new(
            EventId::from(id),
            event.name(),
            event,
            Some(KnownEventVersion::Nothing),
        )
    }

    pub fn update(
        id: RemoteAccountId,
        icon_id: Option<ImageId>,
    ) -> CommandEnvelope<RemoteAccountEvent, RemoteAccount> {
        let event = RemoteAccountEvent::Updated { icon_id };
        CommandEnvelope::new(EventId::from(id), event.name(), event, None)
    }

    pub fn delete(id: RemoteAccountId) -> CommandEnvelope<RemoteAccountEvent, RemoteAccount> {
        let event = RemoteAccountEvent::Deleted;
        CommandEnvelope::new(EventId::from(id), event.name(), event, None)
    }
}

impl EventApplier for RemoteAccount {
    type Event = RemoteAccountEvent;
    const ENTITY_NAME: &'static str = "RemoteAccount";

    fn apply(
        entity: &mut Option<Self>,
        event: EventEnvelope<Self::Event, Self>,
    ) -> error_stack::Result<(), KernelError>
    where
        Self: Sized,
    {
        match event.event {
            RemoteAccountEvent::Created { acct, url, icon_id } => {
                if let Some(entity) = entity {
                    return Err(Report::new(KernelError::Internal)
                        .attach_printable(Self::already_exists(entity)));
                }
                *entity = Some(RemoteAccount {
                    id: RemoteAccountId::new(event.id),
                    acct,
                    url,
                    icon_id,
                });
            }
            RemoteAccountEvent::Updated { icon_id } => {
                if let Some(entity) = entity {
                    entity.icon_id = icon_id;
                } else {
                    return Err(Report::new(KernelError::Internal)
                        .attach_printable(Self::not_exists(event.id.as_ref())));
                }
            }
            RemoteAccountEvent::Deleted => {
                *entity = None;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use crate::entity::{
        CreatedAt, EventEnvelope, EventVersion, ImageId, RemoteAccount, RemoteAccountAcct,
        RemoteAccountId, RemoteAccountUrl,
    };
    use crate::event::EventApplier;
    use uuid::Uuid;

    #[test]
    fn create_remote_account() {
        let id = RemoteAccountId::new(Uuid::now_v7());
        let acct = RemoteAccountAcct::new("acct:".to_string());
        let url = RemoteAccountUrl::new("https://example.com".to_string());
        let create = RemoteAccount::create(id.clone(), acct.clone(), url.clone(), None);
        let envelope = EventEnvelope::new(
            create.id().clone(),
            create.event().clone(),
            EventVersion::new(Uuid::now_v7()),
            CreatedAt::now(),
        );
        let mut entity = None;
        RemoteAccount::apply(&mut entity, envelope).unwrap();
        assert!(entity.is_some());
        let entity = entity.unwrap();
        assert_eq!(entity.id(), &id);
        assert_eq!(entity.acct(), &acct);
        assert_eq!(entity.url(), &url);
        assert!(entity.icon_id().is_none());
    }

    #[test]
    fn update_remote_account() {
        let id = RemoteAccountId::new(Uuid::now_v7());
        let acct = RemoteAccountAcct::new("acct:".to_string());
        let url = RemoteAccountUrl::new("https://example.com".to_string());
        let remote_account = RemoteAccount::new(id.clone(), acct.clone(), url.clone(), None);
        let new_icon_id = Some(ImageId::new(Uuid::now_v7()));
        let update = RemoteAccount::update(id.clone(), new_icon_id);
        let envelope = EventEnvelope::new(
            update.id().clone(),
            update.event().clone(),
            EventVersion::new(Uuid::now_v7()),
            CreatedAt::now(),
        );
        let mut entity = Some(remote_account);
        RemoteAccount::apply(&mut entity, envelope).unwrap();
        assert!(entity.is_some());
        let entity = entity.unwrap();
        assert_eq!(entity.id(), &id);
        assert_eq!(entity.acct(), &acct);
        assert_eq!(entity.url(), &url);
        assert!(entity.icon_id().is_some());
    }

    #[test]
    fn delete_remote_account() {
        let id = RemoteAccountId::new(Uuid::now_v7());
        let acct = RemoteAccountAcct::new("acct:".to_string());
        let url = RemoteAccountUrl::new("https://example.com".to_string());
        let remote_account = RemoteAccount::new(id.clone(), acct.clone(), url.clone(), None);
        let delete = RemoteAccount::delete(id.clone());
        let envelope = EventEnvelope::new(
            delete.id().clone(),
            delete.event().clone(),
            EventVersion::new(Uuid::now_v7()),
            CreatedAt::now(),
        );
        let mut entity = Some(remote_account);
        RemoteAccount::apply(&mut entity, envelope).unwrap();
        assert!(entity.is_none());
    }
}
