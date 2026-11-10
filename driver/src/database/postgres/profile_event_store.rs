use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use error_stack::Report;
use kernel::interfaces::event_store::{DependOnProfileEventStore, ProfileEventStore};
use kernel::prelude::entity::{
    CommandEnvelope, EventEnvelope, EventId, EventVersion, ExpectedVersion, Profile, ProfileEvent,
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

/// Unique indexes created by `20261105000001_profile_username_and_actor_keys.sql`.
const USERNAME_UNIQUE_INDEX: &str = "profile_events_username_unique";
const USERNAME_ONCE_INDEX: &str = "profile_events_username_once_per_profile";

/// Username uniqueness (also against deleted Profiles) and the once-per-Profile rule are
/// enforced by unique indexes over the append-only `profile_events` table, so concurrent
/// assignments race on the database instead of on application-level checks.
fn map_insert_error(error: sqlx::Error) -> Report<KernelError> {
    if let sqlx::Error::Database(db_error) = &error {
        match db_error.constraint() {
            Some(USERNAME_UNIQUE_INDEX) => {
                return Report::new(KernelError::Concurrency)
                    .attach_printable("Profile username is already taken");
            }
            Some(USERNAME_ONCE_INDEX) => {
                return Report::new(KernelError::Concurrency)
                    .attach_printable("Profile already has a username; it is set only once");
            }
            _ => {}
        }
    }
    match error {
        sqlx::Error::PoolTimedOut => Report::from(error).change_context(KernelError::Timeout),
        _ => Report::from(error).change_context(KernelError::Internal),
    }
}

impl ProfileEventStore for PostgresProfileEventStore {
    type Connection = PostgresConnection;

    async fn find_by_id(
        &self,
        executor: &mut Self::Connection,
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
        executor: &mut Self::Connection,
        command: &CommandEnvelope<ProfileEvent, Profile>,
    ) -> error_stack::Result<(), KernelError> {
        self.persist_internal(executor, command, kernel::generate_id())
            .await
    }

    async fn persist_and_transform(
        &self,
        executor: &mut Self::Connection,
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
            Some(ExpectedVersion::Nothing) => {
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
                .map_err(map_insert_error)?
            }
            Some(ExpectedVersion::At(prev)) => {
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
                .map_err(map_insert_error)?
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
                .map_err(map_insert_error)?
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
            let mut conn = db.connection().await.unwrap();
            let profile_id = ProfileId::new(kernel::generate_id());
            let event_id = EventId::from(profile_id.clone());
            let events = db
                .profile_event_store()
                .find_by_id(&mut conn, &event_id, None)
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
                .persist(&mut conn, &created_profile)
                .await
                .unwrap();
            db.profile_event_store()
                .persist(&mut conn, &updated_profile)
                .await
                .unwrap();
            let events = db
                .profile_event_store()
                .find_by_id(&mut conn, &event_id, None)
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
            let mut conn = db.connection().await.unwrap();
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
                .persist(&mut conn, &created_profile)
                .await
                .unwrap();
            db.profile_event_store()
                .persist(&mut conn, &updated_profile)
                .await
                .unwrap();

            // Get all events to obtain the first version
            let all_events = db
                .profile_event_store()
                .find_by_id(&mut conn, &event_id, None)
                .await
                .unwrap();
            assert_eq!(all_events.len(), 2);

            // Query since the first event's version — should return the 2nd event
            let since_events = db
                .profile_event_store()
                .find_by_id(&mut conn, &event_id, Some(&all_events[0].version))
                .await
                .unwrap();
            assert_eq!(since_events.len(), 1);
            assert_eq!(&since_events[0].event, updated_profile.event());

            // Query since the last event's version — should return no events
            let no_events = db
                .profile_event_store()
                .find_by_id(&mut conn, &event_id, Some(&all_events[1].version))
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
            let mut conn = db.connection().await.unwrap();
            let profile_id = ProfileId::new(kernel::generate_id());
            let created_profile = profile_create_command(profile_id.clone());
            db.profile_event_store()
                .persist(&mut conn, &created_profile)
                .await
                .unwrap();
            let events = db
                .profile_event_store()
                .find_by_id(&mut conn, &EventId::from(profile_id), None)
                .await
                .unwrap();
            assert_eq!(events.len(), 1);
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn persist_and_transform_test() {
            kernel::ensure_generator_initialized();
            let db = PostgresDatabase::new().await.unwrap();
            let mut conn = db.connection().await.unwrap();
            let profile_id = ProfileId::new(kernel::generate_id());
            let created_profile = profile_create_command(profile_id.clone());

            let event_envelope = db
                .profile_event_store()
                .persist_and_transform(&mut conn, created_profile.clone())
                .await
                .unwrap();

            assert_eq!(event_envelope.id, EventId::from(profile_id.clone()));
            assert_eq!(&event_envelope.event, created_profile.event());

            let events = db
                .profile_event_store()
                .find_by_id(&mut conn, &EventId::from(profile_id), None)
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
            let mut conn = db.connection().await.unwrap();
            let profile_id = ProfileId::new(kernel::generate_id());
            let created_profile = profile_create_command(profile_id.clone());

            // First persist should succeed
            db.profile_event_store()
                .persist(&mut conn, &created_profile)
                .await
                .unwrap();

            // Second persist with ExpectedVersion::Nothing should fail (concurrency error)
            let result = db
                .profile_event_store()
                .persist(&mut conn, &created_profile)
                .await;
            assert!(result.is_err());
        }
    }
}

#[cfg(test)]
mod username_constraint_tests {
    use crate::database::PostgresDatabase;
    use kernel::interfaces::database::DatabaseConnection;
    use kernel::interfaces::event_store::{DependOnProfileEventStore, ProfileEventStore};
    use kernel::prelude::entity::{
        AccountId, EventId, Nanoid, Profile, ProfileId, ProfileUsername,
    };
    use kernel::KernelError;

    fn unique_username() -> ProfileUsername {
        ProfileUsername::new(format!("u{}", kernel::generate_id()))
    }

    async fn create(db: &PostgresDatabase, username: Option<&ProfileUsername>) -> ProfileId {
        let id = ProfileId::new(kernel::generate_id());
        let mut conn = db.connection().await.unwrap();
        db.profile_event_store()
            .persist(
                &mut conn,
                &Profile::create_with_username(
                    id.clone(),
                    AccountId::default(),
                    None,
                    None,
                    None,
                    None,
                    Nanoid::default(),
                    username.cloned(),
                ),
            )
            .await
            .unwrap();
        id
    }

    async fn set(
        db: &PostgresDatabase,
        id: &ProfileId,
        username: &ProfileUsername,
    ) -> error_stack::Result<(), KernelError> {
        let mut conn = db.connection().await.unwrap();
        db.profile_event_store()
            .persist(
                &mut conn,
                &Profile::set_username(id.clone(), username.clone()),
            )
            .await
    }

    fn is_conflict(result: error_stack::Result<(), KernelError>) -> bool {
        matches!(result, Err(e) if e.current_context() == &KernelError::Concurrency)
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn username_is_unique_across_profiles() {
        kernel::ensure_generator_initialized();
        let db = PostgresDatabase::new().await.unwrap();
        let taken = unique_username();
        let first = create(&db, Some(&taken)).await;

        let second = create(&db, None).await;
        assert!(is_conflict(set(&db, &second, &taken).await));

        let mut conn = db.connection().await.unwrap();
        let duplicate_at_creation = db
            .profile_event_store()
            .persist(
                &mut conn,
                &Profile::create_with_username(
                    ProfileId::new(kernel::generate_id()),
                    AccountId::default(),
                    None,
                    None,
                    None,
                    None,
                    Nanoid::default(),
                    Some(taken.clone()),
                ),
            )
            .await;
        assert!(is_conflict(duplicate_at_creation));

        // A rejected duplicate leaves no event behind, and another value is still accepted.
        let events = db
            .profile_event_store()
            .find_by_id(&mut conn, &EventId::from(second.clone()), None)
            .await
            .unwrap();
        assert_eq!(events.len(), 1);
        assert!(set(&db, &second, &unique_username()).await.is_ok());
        assert_eq!(
            db.profile_event_store()
                .find_by_id(&mut conn, &EventId::from(first), None)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn a_profile_gets_a_username_event_at_most_once() {
        kernel::ensure_generator_initialized();
        let db = PostgresDatabase::new().await.unwrap();

        let unnamed = create(&db, None).await;
        assert!(set(&db, &unnamed, &unique_username()).await.is_ok());
        assert!(is_conflict(set(&db, &unnamed, &unique_username()).await));

        let named_at_creation = create(&db, Some(&unique_username())).await;
        assert!(is_conflict(
            set(&db, &named_at_creation, &unique_username()).await
        ));
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn legacy_created_events_without_a_username_key_still_load() {
        kernel::ensure_generator_initialized();
        let db = PostgresDatabase::new().await.unwrap();
        let id = ProfileId::new(kernel::generate_id());
        let mut conn = db.connection().await.unwrap();
        let con: &mut sqlx::PgConnection = &mut conn;
        sqlx::query(
            "INSERT INTO profile_events (version, id, event_name, data) VALUES ($1, $2, 'profile_created', $3)",
        )
        .bind(kernel::generate_id())
        .bind(id.as_ref())
        .bind(serde_json::json!({
            "type": "created",
            "account_id": kernel::generate_id(),
            "display_name": null,
            "summary": null,
            "icon": null,
            "banner": null,
            "nanoid": "legacy-nanoid",
        }))
        .execute(con)
        .await
        .unwrap();
        let events = db
            .profile_event_store()
            .find_by_id(&mut conn, &EventId::from(id), None)
            .await
            .unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0].event,
            kernel::prelude::entity::ProfileEvent::Created { username: None, .. }
        ));
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn concurrent_assignments_of_one_username_let_exactly_one_win() {
        kernel::ensure_generator_initialized();
        let db = PostgresDatabase::new().await.unwrap();
        let shared = unique_username();
        let mut handles = Vec::new();
        for _ in 0..8 {
            let db = db.clone();
            let shared = shared.clone();
            handles.push(tokio::spawn(async move {
                let id = create(&db, None).await;
                set(&db, &id, &shared).await
            }));
        }
        let mut winners = 0;
        for handle in handles {
            match handle.await.unwrap() {
                Ok(()) => winners += 1,
                Err(error) => assert_eq!(error.current_context(), &KernelError::Concurrency),
            }
        }
        assert_eq!(winners, 1);
    }
}
