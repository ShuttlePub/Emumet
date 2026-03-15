use crate::database::postgres::{CountRow, VersionRow};
use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use error_stack::Report;
use kernel::interfaces::event_store::{DependOnMetadataEventStore, MetadataEventStore};
use kernel::prelude::entity::{
    CommandEnvelope, EventEnvelope, EventId, EventVersion, KnownEventVersion, Metadata,
    MetadataEvent,
};
use kernel::KernelError;
use serde_json;
use sqlx::PgConnection;

#[derive(sqlx::FromRow)]
struct EventRow {
    version: i64,
    id: i64,
    #[allow(dead_code)]
    event_name: String,
    data: serde_json::Value,
}

impl TryFrom<EventRow> for EventEnvelope<MetadataEvent, Metadata> {
    type Error = Report<KernelError>;
    fn try_from(value: EventRow) -> Result<Self, Self::Error> {
        let event: MetadataEvent = serde_json::from_value(value.data).convert_error()?;
        Ok(EventEnvelope::new(
            EventId::new(value.id),
            event,
            EventVersion::new(value.version),
        ))
    }
}

pub struct PostgresMetadataEventStore;

impl MetadataEventStore for PostgresMetadataEventStore {
    type Executor = PostgresConnection;

    async fn find_by_id(
        &self,
        executor: &mut Self::Executor,
        id: &EventId<MetadataEvent, Metadata>,
        since: Option<&EventVersion<Metadata>>,
    ) -> error_stack::Result<Vec<EventEnvelope<MetadataEvent, Metadata>>, KernelError> {
        let con: &mut PgConnection = executor;
        let rows = if let Some(version) = since {
            sqlx::query_as::<_, EventRow>(
                //language=postgresql
                r#"
                SELECT version, id, event_name, data
                FROM metadata_events
                WHERE id = $1 AND version > $2
                ORDER BY version
                "#,
            )
            .bind(id.as_ref())
            .bind(version.as_ref())
            .fetch_all(con)
            .await
            .convert_error()?
        } else {
            sqlx::query_as::<_, EventRow>(
                //language=postgresql
                r#"
                SELECT version, id, event_name, data
                FROM metadata_events
                WHERE id = $1
                ORDER BY version
                "#,
            )
            .bind(id.as_ref())
            .fetch_all(con)
            .await
            .convert_error()?
        };
        rows.into_iter()
            .map(|row| row.try_into())
            .collect::<error_stack::Result<Vec<_>, KernelError>>()
    }

    async fn persist(
        &self,
        executor: &mut Self::Executor,
        command: &CommandEnvelope<MetadataEvent, Metadata>,
    ) -> error_stack::Result<(), KernelError> {
        self.persist_internal(executor, command, kernel::generate_id())
            .await
    }

    async fn persist_and_transform(
        &self,
        executor: &mut Self::Executor,
        command: CommandEnvelope<MetadataEvent, Metadata>,
    ) -> error_stack::Result<EventEnvelope<MetadataEvent, Metadata>, KernelError> {
        let version = kernel::generate_id();
        self.persist_internal(executor, &command, version).await?;

        let command = command.into_destruct();
        Ok(EventEnvelope::new(
            command.id,
            command.event,
            EventVersion::new(version),
        ))
    }
}

impl PostgresMetadataEventStore {
    async fn persist_internal(
        &self,
        executor: &mut PostgresConnection,
        command: &CommandEnvelope<MetadataEvent, Metadata>,
        version: i64,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;

        let event_name = command.event_name();
        let prev_version = command.prev_version().as_ref();
        if let Some(prev_version) = prev_version {
            match prev_version {
                KnownEventVersion::Nothing => {
                    let amount = sqlx::query_as::<_, CountRow>(
                        //language=postgresql
                        r#"
                        SELECT COUNT(*)
                        FROM metadata_events
                        WHERE id = $1
                        "#,
                    )
                    .bind(command.id().as_ref())
                    .fetch_one(&mut *con)
                    .await
                    .convert_error()?;
                    if amount.count != 0 {
                        return Err(Report::new(KernelError::Concurrency).attach_printable(
                            format!("Event {} already exists", command.id().as_ref()),
                        ));
                    }
                }
                KnownEventVersion::Prev(prev_version) => {
                    let last_version = sqlx::query_as::<_, VersionRow>(
                        //language=postgresql
                        r#"
                        SELECT version
                        FROM metadata_events
                        WHERE id = $1
                        ORDER BY version DESC
                        LIMIT 1
                        "#,
                    )
                    .bind(command.id().as_ref())
                    .fetch_optional(&mut *con)
                    .await
                    .convert_error()?;
                    if last_version
                        .map(|row: VersionRow| &row.version != prev_version.as_ref())
                        .unwrap_or(true)
                    {
                        return Err(Report::new(KernelError::Concurrency).attach_printable(
                            format!(
                                "Event {} version {} already exists",
                                command.id().as_ref(),
                                prev_version.as_ref()
                            ),
                        ));
                    }
                }
            };
        }

        sqlx::query(
            //language=postgresql
            r#"
            INSERT INTO metadata_events (version, id, event_name, data)
            VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(version)
        .bind(command.id().as_ref())
        .bind(event_name)
        .bind(serde_json::to_value(command.event()).convert_error()?)
        .execute(con)
        .await
        .convert_error()?;

        Ok(())
    }
}

impl DependOnMetadataEventStore for PostgresDatabase {
    type MetadataEventStore = PostgresMetadataEventStore;

    fn metadata_event_store(&self) -> &Self::MetadataEventStore {
        &PostgresMetadataEventStore
    }
}

#[cfg(test)]
mod test {
    mod query {
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::event_store::{DependOnMetadataEventStore, MetadataEventStore};
        use kernel::prelude::entity::{
            CommandEnvelope, EventId, MetadataContent, MetadataEvent, MetadataId, MetadataLabel,
        };
        use kernel::test_utils::metadata_create_command;

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_id() {
            kernel::ensure_generator_initialized();
            let db = PostgresDatabase::new().await.unwrap();
            let mut transaction = db.begin_transaction().await.unwrap();
            let metadata_id = MetadataId::new(kernel::generate_id());
            let event_id = EventId::from(metadata_id.clone());
            let events = db
                .metadata_event_store()
                .find_by_id(&mut transaction, &event_id, None)
                .await
                .unwrap();
            assert_eq!(events.len(), 0);
            let created_metadata = metadata_create_command(metadata_id.clone());
            let update_event = MetadataEvent::Updated {
                label: MetadataLabel::new("new_label".to_string()),
                content: MetadataContent::new("new_content".to_string()),
            };
            let updated_metadata = CommandEnvelope::new(
                EventId::from(metadata_id.clone()),
                update_event.name(),
                update_event,
                None,
            );
            let delete_event = MetadataEvent::Deleted;
            let deleted_metadata = CommandEnvelope::new(
                EventId::from(metadata_id.clone()),
                delete_event.name(),
                delete_event,
                None,
            );

            db.metadata_event_store()
                .persist(&mut transaction, &created_metadata)
                .await
                .unwrap();
            db.metadata_event_store()
                .persist(&mut transaction, &updated_metadata)
                .await
                .unwrap();
            db.metadata_event_store()
                .persist(&mut transaction, &deleted_metadata)
                .await
                .unwrap();
            let events = db
                .metadata_event_store()
                .find_by_id(&mut transaction, &event_id, None)
                .await
                .unwrap();
            assert_eq!(events.len(), 3);
            assert_eq!(&events[0].event, created_metadata.event());
            assert_eq!(&events[1].event, updated_metadata.event());
            assert_eq!(&events[2].event, deleted_metadata.event());
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_id_since_version() {
            kernel::ensure_generator_initialized();
            let db = PostgresDatabase::new().await.unwrap();
            let mut transaction = db.begin_transaction().await.unwrap();
            let metadata_id = MetadataId::new(kernel::generate_id());
            let event_id = EventId::from(metadata_id.clone());

            let created_metadata = metadata_create_command(metadata_id.clone());
            let update_event = MetadataEvent::Updated {
                label: MetadataLabel::new("new_label".to_string()),
                content: MetadataContent::new("new_content".to_string()),
            };
            let updated_metadata = CommandEnvelope::new(
                EventId::from(metadata_id.clone()),
                update_event.name(),
                update_event,
                None,
            );
            let delete_event = MetadataEvent::Deleted;
            let deleted_metadata = CommandEnvelope::new(
                EventId::from(metadata_id.clone()),
                delete_event.name(),
                delete_event,
                None,
            );

            db.metadata_event_store()
                .persist(&mut transaction, &created_metadata)
                .await
                .unwrap();
            db.metadata_event_store()
                .persist(&mut transaction, &updated_metadata)
                .await
                .unwrap();
            db.metadata_event_store()
                .persist(&mut transaction, &deleted_metadata)
                .await
                .unwrap();

            // Get all events to obtain the first version
            let all_events = db
                .metadata_event_store()
                .find_by_id(&mut transaction, &event_id, None)
                .await
                .unwrap();
            assert_eq!(all_events.len(), 3);

            // Query since the first event's version — should return the 2nd and 3rd events
            let since_events = db
                .metadata_event_store()
                .find_by_id(&mut transaction, &event_id, Some(&all_events[0].version))
                .await
                .unwrap();
            assert_eq!(since_events.len(), 2);
            assert_eq!(&since_events[0].event, updated_metadata.event());
            assert_eq!(&since_events[1].event, deleted_metadata.event());

            // Query since the last event's version — should return no events
            let no_events = db
                .metadata_event_store()
                .find_by_id(&mut transaction, &event_id, Some(&all_events[2].version))
                .await
                .unwrap();
            assert_eq!(no_events.len(), 0);
        }
    }

    mod persist {
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::event_store::{DependOnMetadataEventStore, MetadataEventStore};
        use kernel::prelude::entity::{EventId, MetadataId};
        use kernel::test_utils::metadata_create_command;

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn basic_creation() {
            kernel::ensure_generator_initialized();
            let db = PostgresDatabase::new().await.unwrap();
            let mut transaction = db.begin_transaction().await.unwrap();
            let metadata_id = MetadataId::new(kernel::generate_id());
            let created_metadata = metadata_create_command(metadata_id.clone());
            db.metadata_event_store()
                .persist(&mut transaction, &created_metadata)
                .await
                .unwrap();
            let events = db
                .metadata_event_store()
                .find_by_id(&mut transaction, &EventId::from(metadata_id), None)
                .await
                .unwrap();
            assert_eq!(events.len(), 1);
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn persist_and_transform_test() {
            kernel::ensure_generator_initialized();
            let db = PostgresDatabase::new().await.unwrap();
            let mut transaction = db.begin_transaction().await.unwrap();
            let metadata_id = MetadataId::new(kernel::generate_id());
            let created_metadata = metadata_create_command(metadata_id.clone());

            let event_envelope = db
                .metadata_event_store()
                .persist_and_transform(&mut transaction, created_metadata.clone())
                .await
                .unwrap();

            assert_eq!(event_envelope.id, EventId::from(metadata_id.clone()));
            assert_eq!(&event_envelope.event, created_metadata.event());

            let events = db
                .metadata_event_store()
                .find_by_id(&mut transaction, &EventId::from(metadata_id), None)
                .await
                .unwrap();
            assert_eq!(events.len(), 1);
            assert_eq!(&events[0].event, created_metadata.event());
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn known_event_version_nothing_prevents_duplicate() {
            kernel::ensure_generator_initialized();
            let db = PostgresDatabase::new().await.unwrap();
            let mut transaction = db.begin_transaction().await.unwrap();
            let metadata_id = MetadataId::new(kernel::generate_id());
            let created_metadata = metadata_create_command(metadata_id.clone());

            // First persist should succeed
            db.metadata_event_store()
                .persist(&mut transaction, &created_metadata)
                .await
                .unwrap();

            // Second persist with KnownEventVersion::Nothing should fail (concurrency error)
            let result = db
                .metadata_event_store()
                .persist(&mut transaction, &created_metadata)
                .await;
            assert!(result.is_err());
        }
    }
}
