mod content;
mod id;
mod label;

pub use self::content::*;
pub use self::id::*;
pub use self::label::*;
use super::{AccountId, CommandEnvelope, CreatedAt, EventEnvelope, EventId, KnownEventVersion};
use crate::event::EventApplier;
use crate::KernelError;
use destructure::Destructure;
use error_stack::Report;
use serde::{Deserialize, Serialize};
use vodca::{Nameln, Newln, References};

#[derive(Debug, Clone, Eq, PartialEq, References, Newln, Destructure, Serialize, Deserialize)]
pub struct Metadata {
    id: MetadataId,
    account_id: AccountId,
    label: MetadataLabel,
    content: MetadataContent,
    created_at: CreatedAt<Metadata>,
}

#[derive(Debug, Clone, Nameln, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[vodca(prefix = "metadata", snake_case)]
pub enum MetadataEvent {
    Created {
        account_id: AccountId,
        label: MetadataLabel,
        content: MetadataContent,
    },
    Updated {
        label: MetadataLabel,
        content: MetadataContent,
    },
    Deleted,
}

impl Metadata {
    pub fn create(
        id: MetadataId,
        account_id: AccountId,
        label: MetadataLabel,
        content: MetadataContent,
    ) -> CommandEnvelope<MetadataEvent, Metadata> {
        let event = MetadataEvent::Created {
            account_id,
            label,
            content,
        };
        CommandEnvelope::new(
            EventId::from(id),
            event.name(),
            event,
            Some(KnownEventVersion::Nothing),
        )
    }

    pub fn update(
        id: MetadataId,
        label: MetadataLabel,
        content: MetadataContent,
    ) -> CommandEnvelope<MetadataEvent, Metadata> {
        let event = MetadataEvent::Updated { label, content };
        CommandEnvelope::new(EventId::from(id), event.name(), event, None)
    }

    pub fn delete(id: MetadataId) -> CommandEnvelope<MetadataEvent, Metadata> {
        let event = MetadataEvent::Deleted;
        CommandEnvelope::new(EventId::from(id), event.name(), event, None)
    }
}

impl EventApplier for Metadata {
    type Event = MetadataEvent;
    const ENTITY_NAME: &'static str = "Metadata";

    fn apply(
        entity: &mut Option<Self>,
        event: EventEnvelope<Self::Event, Self>,
    ) -> error_stack::Result<(), KernelError>
    where
        Self: Sized,
    {
        match event.event {
            MetadataEvent::Created {
                account_id,
                label,
                content,
            } => {
                if let Some(entity) = entity {
                    return Err(Report::new(KernelError::Internal)
                        .attach_printable(Self::already_exists(entity)));
                }
                *entity = Some(Metadata {
                    id: MetadataId::new(event.id),
                    account_id,
                    label,
                    content,
                    created_at: event.created_at,
                });
            }
            MetadataEvent::Updated { label, content } => {
                if let Some(entity) = entity {
                    entity.label = label;
                    entity.content = content;
                } else {
                    return Err(Report::new(KernelError::Internal)
                        .attach_printable(Self::not_exists(event.id.as_ref())));
                }
            }
            MetadataEvent::Deleted => {
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
        AccountId, CreatedAt, EventEnvelope, EventVersion, Metadata, MetadataContent, MetadataId,
        MetadataLabel,
    };
    use crate::event::EventApplier;
    use uuid::Uuid;

    #[test]
    fn create_metadata() {
        let account_id = AccountId::new(Uuid::now_v7());
        let id = MetadataId::new(Uuid::now_v7());
        let label = MetadataLabel::new("label".to_string());
        let content = MetadataContent::new("content".to_string());
        let create_event = Metadata::create(
            id.clone(),
            account_id.clone(),
            label.clone(),
            content.clone(),
        );
        let envelope = EventEnvelope::new(
            create_event.id().clone().into(),
            create_event.event().clone(),
            EventVersion::new(Uuid::now_v7()),
            CreatedAt::now(),
        );
        let mut metadata = None;
        Metadata::apply(&mut metadata, envelope).unwrap();
        assert!(metadata.is_some());
        let metadata = metadata.unwrap();
        assert_eq!(metadata.id(), &id);
        assert_eq!(metadata.account_id(), &account_id);
        assert_eq!(metadata.label(), &label);
        assert_eq!(metadata.content(), &content);
    }

    #[test]
    fn update_metadata() {
        let account_id = AccountId::new(Uuid::now_v7());
        let id = MetadataId::new(Uuid::now_v7());
        let label = MetadataLabel::new("label".to_string());
        let content = MetadataContent::new("content".to_string());
        let metadata = Metadata::new(
            id.clone(),
            account_id.clone(),
            label.clone(),
            content.clone(),
            CreatedAt::now(),
        );
        let label = MetadataLabel::new("new_label".to_string());
        let content = MetadataContent::new("new_content".to_string());
        let update_event = Metadata::update(id.clone(), label.clone(), content.clone());
        let envelope = EventEnvelope::new(
            update_event.id().clone().into(),
            update_event.event().clone(),
            EventVersion::new(Uuid::now_v7()),
            CreatedAt::now(),
        );
        let mut metadata = Some(metadata);
        Metadata::apply(&mut metadata, envelope).unwrap();
        assert!(metadata.is_some());
        let metadata = metadata.unwrap();
        assert_eq!(metadata.id(), &id);
        assert_eq!(metadata.account_id(), &account_id);
        assert_eq!(metadata.label(), &label);
        assert_eq!(metadata.content(), &content);
    }

    #[test]
    fn delete_metadata() {
        let account_id = AccountId::new(Uuid::now_v7());
        let id = MetadataId::new(Uuid::now_v7());
        let label = MetadataLabel::new("label".to_string());
        let content = MetadataContent::new("content".to_string());
        let metadata = Metadata::new(
            id.clone(),
            account_id.clone(),
            label.clone(),
            content.clone(),
            CreatedAt::now(),
        );
        let delete_event = Metadata::delete(id.clone());
        let envelope = EventEnvelope::new(
            delete_event.id().clone().into(),
            delete_event.event().clone(),
            EventVersion::new(Uuid::now_v7()),
            CreatedAt::now(),
        );
        let mut metadata = Some(metadata);
        Metadata::apply(&mut metadata, envelope).unwrap();
        assert!(metadata.is_none());
    }
}
