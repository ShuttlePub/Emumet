use super::metadata_event_store::PostgresMetadataEventStore;
use crate::database::{PostgresConnection, PostgresDatabase};
use error_stack::Report;
use kernel::interfaces::event_store::MetadataEventStore;
use kernel::interfaces::repository::{AggregateRepository, DependOnMetadataRepository, Rehydrated};
use kernel::prelude::entity::{
    CommandEnvelope, EventEnvelope, EventId, Metadata, MetadataEvent, MetadataId,
};
use kernel::KernelError;

pub struct PostgresMetadataRepository;

impl AggregateRepository<Metadata> for PostgresMetadataRepository {
    type Connection = PostgresConnection;
    type Id = MetadataId;

    async fn load(
        &self,
        executor: &mut Self::Connection,
        id: &Self::Id,
    ) -> error_stack::Result<Rehydrated<Metadata>, KernelError> {
        let events = PostgresMetadataEventStore
            .find_by_id(executor, &EventId::from(id.clone()), None)
            .await?;
        Rehydrated::<Metadata>::from_events_allow_deletion(events)?.ok_or_else(|| {
            Report::new(KernelError::NotFound)
                .attach_printable(format!("No events found for metadata: {}", id.as_ref()))
        })
    }

    async fn save(
        &self,
        executor: &mut Self::Connection,
        command: CommandEnvelope<MetadataEvent, Metadata>,
    ) -> error_stack::Result<EventEnvelope<MetadataEvent, Metadata>, KernelError> {
        PostgresMetadataEventStore
            .persist_and_transform(executor, command)
            .await
    }
}

impl DependOnMetadataRepository for PostgresDatabase {
    type MetadataRepository = PostgresMetadataRepository;

    fn metadata_repository(&self) -> &Self::MetadataRepository {
        &PostgresMetadataRepository
    }
}

#[cfg(test)]
mod test {
    use super::PostgresMetadataRepository;
    use crate::database::PostgresDatabase;
    use kernel::interfaces::database::DatabaseConnection;
    use kernel::interfaces::event_store::{DependOnMetadataEventStore, MetadataEventStore};
    use kernel::interfaces::repository::AggregateRepository;
    use kernel::prelude::entity::{
        EventEnvelope, Metadata, MetadataContent, MetadataEvent, MetadataId, MetadataLabel,
    };
    use kernel::test_utils::MetadataBuilder;
    use kernel::KernelError;

    async fn create_metadata(
        db: &PostgresDatabase,
    ) -> (MetadataId, EventEnvelope<MetadataEvent, Metadata>) {
        let mut conn = db.connection().await.unwrap();
        let metadata = MetadataBuilder::new().build();
        let id = metadata.id().clone();
        let command = Metadata::create(
            id.clone(),
            metadata.account_id().clone(),
            metadata.label().clone(),
            metadata.content().clone(),
            metadata.nanoid().clone(),
        );
        let envelope = db
            .metadata_event_store()
            .persist_and_transform(&mut conn, command)
            .await
            .unwrap();
        (id, envelope)
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn can_load_rehydrated_metadata() {
        let db = PostgresDatabase::new().await.unwrap();
        let (id, _) = create_metadata(&db).await;
        let mut conn = db.connection().await.unwrap();

        let loaded = PostgresMetadataRepository
            .load(&mut conn, &id)
            .await
            .unwrap();
        assert_eq!(loaded.aggregate().id(), &id);
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn can_save_and_update_metadata() {
        let db = PostgresDatabase::new().await.unwrap();
        let (id, _) = create_metadata(&db).await;
        let mut conn = db.connection().await.unwrap();
        let initial = PostgresMetadataRepository
            .load(&mut conn, &id)
            .await
            .unwrap();

        let update_command = Metadata::update(
            id.clone(),
            MetadataLabel::new("updated-label".to_string()),
            MetadataContent::new("updated content".to_string()),
            initial.version().clone(),
        );
        let envelope = PostgresMetadataRepository
            .save(&mut conn, update_command)
            .await
            .unwrap();

        let updated = PostgresMetadataRepository
            .load(&mut conn, &id)
            .await
            .unwrap();
        assert_eq!(
            updated.aggregate().label(),
            &MetadataLabel::new("updated-label".to_string())
        );
        assert_eq!(updated.version(), &envelope.version);
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn save_with_stale_version_fails() {
        let db = PostgresDatabase::new().await.unwrap();
        let (id, _) = create_metadata(&db).await;
        let mut conn = db.connection().await.unwrap();
        let initial = PostgresMetadataRepository
            .load(&mut conn, &id)
            .await
            .unwrap();

        let first_update = Metadata::update(
            id.clone(),
            MetadataLabel::new("first".to_string()),
            MetadataContent::new("first content".to_string()),
            initial.version().clone(),
        );
        PostgresMetadataRepository
            .save(&mut conn, first_update)
            .await
            .unwrap();

        let stale_update = Metadata::update(
            id.clone(),
            MetadataLabel::new("stale".to_string()),
            MetadataContent::new("stale content".to_string()),
            initial.version().clone(),
        );
        let result = PostgresMetadataRepository
            .save(&mut conn, stale_update)
            .await;
        assert!(result.is_err_and(|e| e.current_context() == &KernelError::Concurrency));
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn load_missing_metadata_fails() {
        let db = PostgresDatabase::new().await.unwrap();
        let mut conn = db.connection().await.unwrap();
        let id = MetadataId::new(kernel::generate_id());

        let result = PostgresMetadataRepository.load(&mut conn, &id).await;
        assert!(result.is_err_and(|e| e.current_context() == &KernelError::NotFound));
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn load_deleted_metadata_returns_not_found() {
        let db = PostgresDatabase::new().await.unwrap();
        let mut conn = db.connection().await.unwrap();
        let (id, _) = create_metadata(&db).await;

        // Rehydrate to get the current version, then delete.
        let initial = PostgresMetadataRepository
            .load(&mut conn, &id)
            .await
            .unwrap();
        let delete_command = Metadata::delete(id.clone(), initial.version().clone());
        PostgresMetadataRepository
            .save(&mut conn, delete_command)
            .await
            .unwrap();

        // load should return NotFound, not Internal.
        let result = PostgresMetadataRepository.load(&mut conn, &id).await;
        assert!(result.is_err_and(|e| e.current_context() == &KernelError::NotFound));
    }
}
