mod approved_at;
mod id;
mod target_id;

pub use self::{approved_at::*, id::*, target_id::*};

use crate::entity::{CommandEnvelope, EventEnvelope, EventId};
use crate::event::EventApplier;
use crate::KernelError;
use error_stack::ResultExt;
use serde::{Deserialize, Serialize};
use vodca::{Nameln, References};

#[derive(Debug, Clone, Hash, Eq, PartialEq, References, Serialize, Deserialize)]
pub struct Follow {
    id: FollowId,
    source: FollowTargetId,
    destination: FollowTargetId,
    approved_at: Option<FollowApprovedAt>,
}

impl Follow {
    pub fn new(
        id: FollowId,
        source: FollowTargetId,
        destination: FollowTargetId,
        approved_at: Option<FollowApprovedAt>,
    ) -> error_stack::Result<Self, KernelError> {
        match (source, destination) {
            (source @ FollowTargetId::Remote(_), destination @ FollowTargetId::Remote(_)) => {
                Err(KernelError::Internal).attach_printable(format!(
                    "Cannot create remote to remote follow data. source: {:?}, destination: {:?}",
                    source, destination
                ))
            }
            (source, destination) => Ok(Self {
                id,
                source,
                destination,
                approved_at,
            }),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Nameln, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[vodca(prefix = "follow", snake_case)]
pub enum FollowEvent {
    Created {
        source: FollowTargetId,
        destination: FollowTargetId,
    },
    Approved,
    Deleted,
}

impl Follow {
    pub fn create(
        id: FollowId,
        source: FollowTargetId,
        destination: FollowTargetId,
    ) -> error_stack::Result<CommandEnvelope<FollowEvent, Follow>, KernelError> {
        match (source, destination) {
            (source @ FollowTargetId::Remote(_), destination @ FollowTargetId::Remote(_)) => {
                Err(KernelError::Internal).attach_printable(format!(
                    "Cannot create remote to remote follow data. source: {:?}, destination: {:?}",
                    source, destination
                ))
            }
            (source, destination) => {
                let event = FollowEvent::Created {
                    source,
                    destination,
                };
                Ok(CommandEnvelope::new(
                    EventId::from(id),
                    event.name(),
                    event,
                    None,
                ))
            }
        }
    }

    pub fn approve(id: FollowId) -> CommandEnvelope<FollowEvent, Follow> {
        let event = FollowEvent::Approved;
        CommandEnvelope::new(EventId::from(id), event.name(), event, None)
    }

    pub fn delete(id: FollowId) -> CommandEnvelope<FollowEvent, Follow> {
        let event = FollowEvent::Deleted;
        CommandEnvelope::new(EventId::from(id), event.name(), event, None)
    }
}

impl EventApplier for Follow {
    type Event = FollowEvent;
    const ENTITY_NAME: &'static str = "Follow";

    fn apply(
        entity: &mut Option<Self>,
        event: EventEnvelope<Self::Event, Self>,
    ) -> error_stack::Result<(), KernelError>
    where
        Self: Sized,
    {
        match event.event {
            FollowEvent::Created {
                source,
                destination,
            } => {
                if let Some(entity) = entity {
                    return Err(KernelError::Internal)
                        .attach_printable(Self::already_exists(entity));
                }
                *entity = Some(Follow::new(
                    FollowId::new(event.id),
                    source,
                    destination,
                    None,
                )?);
            }
            FollowEvent::Approved => {
                if let Some(entity) = entity {
                    entity.approved_at = Some(FollowApprovedAt::new(event.created_at));
                } else {
                    return Err(KernelError::Internal)
                        .attach_printable(Self::not_exists(event.id.as_ref()));
                }
            }
            FollowEvent::Deleted => {
                *entity = None;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use crate::entity::{
        AccountId, CreatedAt, EventEnvelope, EventVersion, Follow, FollowId,
        FollowTargetId, RemoteAccountId,
    };
    use crate::event::EventApplier;
    use uuid::Uuid;

    #[test]
    fn create_event() {
        let id = FollowId::new(Uuid::now_v7());
        let source = FollowTargetId::from(AccountId::new(Uuid::now_v7()));
        let destination = FollowTargetId::from(RemoteAccountId::new(Uuid::now_v7()));
        let event = Follow::create(id.clone(), source.clone(), destination.clone()).unwrap();
        let envelope = EventEnvelope::new(
            event.id().clone(),
            event.event().clone(),
            EventVersion::new(Uuid::now_v7()),
            CreatedAt::now(),
        );
        let mut entity = None;
        Follow::apply(&mut entity, envelope).unwrap();
        assert!(entity.is_some());
        let entity = entity.unwrap();
        assert_eq!(entity.id(), &id);
        assert_eq!(entity.source(), &source);
        assert_eq!(entity.destination(), &destination);
        assert!(entity.approved_at().is_none());
    }

    #[test]
    fn update_event() {
        let id = FollowId::new(Uuid::now_v7());
        let source = FollowTargetId::from(AccountId::new(Uuid::now_v7()));
        let destination = FollowTargetId::from(RemoteAccountId::new(Uuid::now_v7()));
        let follow = Follow::new(id.clone(), source.clone(), destination.clone(), None).unwrap();
        let event = Follow::approve(id.clone());
        let envelope = EventEnvelope::new(
            event.id().clone(),
            event.event().clone(),
            EventVersion::new(Uuid::now_v7()),
            CreatedAt::now(),
        );
        let mut entity = Some(follow);
        Follow::apply(&mut entity, envelope).unwrap();
        assert!(entity.is_some());
        let entity = entity.unwrap();
        assert_eq!(entity.id(), &id);
        assert_eq!(entity.source(), &source);
        assert_eq!(entity.destination(), &destination);
        assert!(entity.approved_at().is_some());
    }

    #[test]
    fn delete_event() {
        let id = FollowId::new(Uuid::now_v7());
        let source = FollowTargetId::from(AccountId::new(Uuid::now_v7()));
        let destination = FollowTargetId::from(RemoteAccountId::new(Uuid::now_v7()));
        let follow = Follow::new(id.clone(), source.clone(), destination.clone(), None).unwrap();
        let event = Follow::delete(id.clone());
        let envelope = EventEnvelope::new(
            event.id().clone(),
            event.event().clone(),
            EventVersion::new(Uuid::now_v7()),
            CreatedAt::now(),
        );
        let mut entity = Some(follow);
        Follow::apply(&mut entity, envelope).unwrap();
        assert!(entity.is_none());
    }
}
