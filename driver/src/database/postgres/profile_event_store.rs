use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use error_stack::Report;
use kernel::interfaces::event_store::{DependOnProfileEventStore, ProfileEventStore};
use kernel::prelude::entity::{
    CommandEnvelope, EventEnvelope, EventId, EventVersion, KnownEventVersion, Profile, ProfileEvent,
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

impl TryFrom<EventRow> for EventEnvelope<ProfileEvent, Profile> {
    type Error = Report<KernelError>;
    fn try_from(value: EventRow) -> Result<Self, Self::Error> {
        let event: ProfileEvent = serde_json::from_value(value.data).convert_error()?;
        Ok(EventEnvelope::new(
            EventId::new(value.id),
            event,
            EventVersion::new(value.version),
        ))
    }
}

pub struct PostgresProfileEventStore;

impl ProfileEventStore for PostgresProfileEventStore {
    type Executor = PostgresConnection;

    async fn find_by_id(
        &self,
        executor: &mut Self::Executor,
        id: &EventId<ProfileEvent, Profile>,
        since: Option<&EventVersion<Profile>>,
    ) -> error_stack::Result<Vec<EventEnvelope<ProfileEvent, Profile>>, KernelError> {
        let con: &mut PgConnection = executor;
        let rows = if let Some(version) = since {
            sqlx::query_as::<_, EventRow>(
                //language=postgresql
                r#"
                SELECT version, id, event_name, data
                FROM profile_events
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
                FROM profile_events
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
        command: &CommandEnvelope<ProfileEvent, Profile>,
    ) -> error_stack::Result<(), KernelError> {
        self.persist_internal(executor, command, kernel::generate_id())
            .await
    }

    async fn persist_and_transform(
        &self,
        executor: &mut Self::Executor,
        command: CommandEnvelope<ProfileEvent, Profile>,
    ) -> error_stack::Result<EventEnvelope<ProfileEvent, Profile>, KernelError> {
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

impl PostgresProfileEventStore {
    async fn persist_internal(
        &self,
        executor: &mut PostgresConnection,
        command: &CommandEnvelope<ProfileEvent, Profile>,
        version: i64,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;

        let event_name = command.event_name();
        let data = serde_json::to_value(command.event()).convert_error()?;
        let prev_version = command.prev_version().as_ref();

        let result = match prev_version {
            Some(KnownEventVersion::Nothing) => {
                sqlx::query(
                    //language=postgresql
                    r#"
                    INSERT INTO profile_events (version, id, event_name, data)
                    SELECT $1, $2, $3, $4
                    WHERE NOT EXISTS (SELECT 1 FROM profile_events WHERE id = $2)
                    "#,
                )
                .bind(version)
                .bind(command.id().as_ref())
                .bind(event_name)
                .bind(&data)
                .execute(&mut *con)
                .await
                .convert_error()?
            }
            Some(KnownEventVersion::Prev(prev)) => {
                sqlx::query(
                    //language=postgresql
                    r#"
                    INSERT INTO profile_events (version, id, event_name, data)
                    SELECT $1, $2, $3, $4
                    WHERE (SELECT MAX(version) FROM profile_events WHERE id = $2) = $5
                    "#,
                )
                .bind(version)
                .bind(command.id().as_ref())
                .bind(event_name)
                .bind(&data)
                .bind(prev.as_ref())
                .execute(&mut *con)
                .await
                .convert_error()?
            }
            None => {
                sqlx::query(
                    //language=postgresql
                    r#"
                    INSERT INTO profile_events (version, id, event_name, data)
                    VALUES ($1, $2, $3, $4)
                    "#,
                )
                .bind(version)
                .bind(command.id().as_ref())
                .bind(event_name)
                .bind(&data)
                .execute(con)
                .await
                .convert_error()?
            }
        };

        if prev_version.is_some() && result.rows_affected() == 0 {
            return Err(
                Report::new(KernelError::Concurrency).attach_printable(format!(
                    "Concurrency conflict for event {}",
                    command.id().as_ref()
                )),
            );
        }

        Ok(())
    }
}

impl DependOnProfileEventStore for PostgresDatabase {
    type ProfileEventStore = PostgresProfileEventStore;

    fn profile_event_store(&self) -> &Self::ProfileEventStore {
        &PostgresProfileEventStore
    }
}

#[cfg(test)]
mod test {
    mod query {
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::event_store::{DependOnProfileEventStore, ProfileEventStore};
        use kernel::prelude::entity::{
            CommandEnvelope, EventId, FieldAction, ProfileEvent, ProfileId,
        };
        use kernel::test_utils::profile_create_command;

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_id() {
            kernel::ensure_generator_initialized();
            let db = PostgresDatabase::new().await.unwrap();
            let mut transaction = db.get_executor().await.unwrap();
            let profile_id = ProfileId::new(kernel::generate_id());
            let event_id = EventId::from(profile_id.clone());
            let events = db
                .profile_event_store()
                .find_by_id(&mut transaction, &event_id, None)
                .await
                .unwrap();
            assert_eq!(events.len(), 0);
            let created_profile = profile_create_command(profile_id.clone());
            let update_event = ProfileEvent::Updated {
                display_name: FieldAction::Unchanged,
                summary: FieldAction::Unchanged,
                icon: FieldAction::Unchanged,
                banner: FieldAction::Unchanged,
            };
            let updated_profile = CommandEnvelope::new(
                EventId::from(profile_id.clone()),
                update_event.name(),
                update_event,
                None,
            );

            db.profile_event_store()
                .persist(&mut transaction, &created_profile)
                .await
                .unwrap();
            db.profile_event_store()
                .persist(&mut transaction, &updated_profile)
                .await
                .unwrap();
            let events = db
                .profile_event_store()
                .find_by_id(&mut transaction, &event_id, None)
                .await
                .unwrap();
            assert_eq!(events.len(), 2);
            assert_eq!(&events[0].event, created_profile.event());
            assert_eq!(&events[1].event, updated_profile.event());
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_id_since_version() {
            kernel::ensure_generator_initialized();
            let db = PostgresDatabase::new().await.unwrap();
            let mut transaction = db.get_executor().await.unwrap();
            let profile_id = ProfileId::new(kernel::generate_id());
            let event_id = EventId::from(profile_id.clone());

            let created_profile = profile_create_command(profile_id.clone());
            let update_event = ProfileEvent::Updated {
                display_name: FieldAction::Unchanged,
                summary: FieldAction::Unchanged,
                icon: FieldAction::Unchanged,
                banner: FieldAction::Unchanged,
            };
            let updated_profile = CommandEnvelope::new(
                EventId::from(profile_id.clone()),
                update_event.name(),
                update_event,
                None,
            );

            db.profile_event_store()
                .persist(&mut transaction, &created_profile)
                .await
                .unwrap();
            db.profile_event_store()
                .persist(&mut transaction, &updated_profile)
                .await
                .unwrap();

            // Get all events to obtain the first version
            let all_events = db
                .profile_event_store()
                .find_by_id(&mut transaction, &event_id, None)
                .await
                .unwrap();
            assert_eq!(all_events.len(), 2);

            // Query since the first event's version — should return the 2nd event
            let since_events = db
                .profile_event_store()
                .find_by_id(&mut transaction, &event_id, Some(&all_events[0].version))
                .await
                .unwrap();
            assert_eq!(since_events.len(), 1);
            assert_eq!(&since_events[0].event, updated_profile.event());

            // Query since the last event's version — should return no events
            let no_events = db
                .profile_event_store()
                .find_by_id(&mut transaction, &event_id, Some(&all_events[1].version))
                .await
                .unwrap();
            assert_eq!(no_events.len(), 0);
        }
    }

    mod persist {
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::event_store::{DependOnProfileEventStore, ProfileEventStore};
        use kernel::prelude::entity::{EventId, ProfileId};
        use kernel::test_utils::profile_create_command;

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn basic_creation() {
            kernel::ensure_generator_initialized();
            let db = PostgresDatabase::new().await.unwrap();
            let mut transaction = db.get_executor().await.unwrap();
            let profile_id = ProfileId::new(kernel::generate_id());
            let created_profile = profile_create_command(profile_id.clone());
            db.profile_event_store()
                .persist(&mut transaction, &created_profile)
                .await
                .unwrap();
            let events = db
                .profile_event_store()
                .find_by_id(&mut transaction, &EventId::from(profile_id), None)
                .await
                .unwrap();
            assert_eq!(events.len(), 1);
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn persist_and_transform_test() {
            kernel::ensure_generator_initialized();
            let db = PostgresDatabase::new().await.unwrap();
            let mut transaction = db.get_executor().await.unwrap();
            let profile_id = ProfileId::new(kernel::generate_id());
            let created_profile = profile_create_command(profile_id.clone());

            let event_envelope = db
                .profile_event_store()
                .persist_and_transform(&mut transaction, created_profile.clone())
                .await
                .unwrap();

            assert_eq!(event_envelope.id, EventId::from(profile_id.clone()));
            assert_eq!(&event_envelope.event, created_profile.event());

            let events = db
                .profile_event_store()
                .find_by_id(&mut transaction, &EventId::from(profile_id), None)
                .await
                .unwrap();
            assert_eq!(events.len(), 1);
            assert_eq!(&events[0].event, created_profile.event());
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn known_event_version_nothing_prevents_duplicate() {
            kernel::ensure_generator_initialized();
            let db = PostgresDatabase::new().await.unwrap();
            let mut transaction = db.get_executor().await.unwrap();
            let profile_id = ProfileId::new(kernel::generate_id());
            let created_profile = profile_create_command(profile_id.clone());

            // First persist should succeed
            db.profile_event_store()
                .persist(&mut transaction, &created_profile)
                .await
                .unwrap();

            // Second persist with KnownEventVersion::Nothing should fail (concurrency error)
            let result = db
                .profile_event_store()
                .persist(&mut transaction, &created_profile)
                .await;
            assert!(result.is_err());
        }
    }
}
