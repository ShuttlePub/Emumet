use crate::database::postgres::{CountRow, VersionRow};
use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use error_stack::Report;
use kernel::interfaces::event_store::{AuthAccountEventStore, DependOnAuthAccountEventStore};
use kernel::prelude::entity::{
    AuthAccount, AuthAccountEvent, CommandEnvelope, EventEnvelope, EventId, EventVersion,
    KnownEventVersion,
};
use kernel::KernelError;
use serde_json;
use sqlx::PgConnection;
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct EventRow {
    version: Uuid,
    id: Uuid,
    #[allow(dead_code)]
    event_name: String,
    data: serde_json::Value,
}

impl TryFrom<EventRow> for EventEnvelope<AuthAccountEvent, AuthAccount> {
    type Error = Report<KernelError>;
    fn try_from(value: EventRow) -> Result<Self, Self::Error> {
        let event: AuthAccountEvent = serde_json::from_value(value.data).convert_error()?;
        Ok(EventEnvelope::new(
            EventId::new(value.id),
            event,
            EventVersion::new(value.version),
        ))
    }
}

pub struct PostgresAuthAccountEventStore;

impl AuthAccountEventStore for PostgresAuthAccountEventStore {
    type Executor = PostgresConnection;

    async fn find_by_id(
        &self,
        executor: &mut Self::Executor,
        id: &EventId<AuthAccountEvent, AuthAccount>,
        since: Option<&EventVersion<AuthAccount>>,
    ) -> error_stack::Result<Vec<EventEnvelope<AuthAccountEvent, AuthAccount>>, KernelError> {
        let con: &mut PgConnection = executor;
        let rows = if let Some(version) = since {
            sqlx::query_as::<_, EventRow>(
                //language=postgresql
                r#"
                SELECT version, id, event_name, data
                FROM auth_account_events
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
                FROM auth_account_events
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
        command: &CommandEnvelope<AuthAccountEvent, AuthAccount>,
    ) -> error_stack::Result<(), KernelError> {
        self.persist_internal(executor, command, Uuid::now_v7())
            .await
    }

    async fn persist_and_transform(
        &self,
        executor: &mut Self::Executor,
        command: CommandEnvelope<AuthAccountEvent, AuthAccount>,
    ) -> error_stack::Result<EventEnvelope<AuthAccountEvent, AuthAccount>, KernelError> {
        let version = Uuid::now_v7();
        self.persist_internal(executor, &command, version).await?;

        let command = command.into_destruct();
        Ok(EventEnvelope::new(
            command.id,
            command.event,
            EventVersion::new(version),
        ))
    }
}

impl PostgresAuthAccountEventStore {
    async fn persist_internal(
        &self,
        executor: &mut PostgresConnection,
        command: &CommandEnvelope<AuthAccountEvent, AuthAccount>,
        version: Uuid,
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
                        FROM auth_account_events
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
                        FROM auth_account_events
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
            INSERT INTO auth_account_events (version, id, event_name, data)
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

impl DependOnAuthAccountEventStore for PostgresDatabase {
    type AuthAccountEventStore = PostgresAuthAccountEventStore;

    fn auth_account_event_store(&self) -> &Self::AuthAccountEventStore {
        &PostgresAuthAccountEventStore
    }
}

#[cfg(test)]
mod test {
    use crate::database::PostgresDatabase;
    use kernel::interfaces::database::DatabaseConnection;
    use kernel::interfaces::event_store::{AuthAccountEventStore, DependOnAuthAccountEventStore};
    use kernel::prelude::entity::{
        AuthAccount, AuthAccountClientId, AuthAccountEvent, AuthAccountId, AuthHostId,
        CommandEnvelope, EventId,
    };
    use uuid::Uuid;

    fn create_auth_account_command(
        id: AuthAccountId,
    ) -> CommandEnvelope<AuthAccountEvent, AuthAccount> {
        AuthAccount::create(
            id,
            AuthHostId::new(Uuid::now_v7()),
            AuthAccountClientId::new("test_client"),
        )
    }

    mod query {
        use super::*;

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_id() {
            let db = PostgresDatabase::new().await.unwrap();
            let mut transaction = db.begin_transaction().await.unwrap();
            let id = AuthAccountId::new(Uuid::now_v7());
            let event_id = EventId::from(id.clone());
            let events = db
                .auth_account_event_store()
                .find_by_id(&mut transaction, &event_id, None)
                .await
                .unwrap();
            assert_eq!(events.len(), 0);

            let created = create_auth_account_command(id.clone());
            db.auth_account_event_store()
                .persist_and_transform(&mut transaction, created.clone())
                .await
                .unwrap();

            let events = db
                .auth_account_event_store()
                .find_by_id(&mut transaction, &event_id, None)
                .await
                .unwrap();
            assert_eq!(events.len(), 1);
            assert_eq!(&events[0].event, created.event());
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_id_since_version() {
            let db = PostgresDatabase::new().await.unwrap();
            let mut transaction = db.begin_transaction().await.unwrap();
            let id = AuthAccountId::new(Uuid::now_v7());
            let event_id = EventId::from(id.clone());

            let created = create_auth_account_command(id.clone());
            let create_envelope = db
                .auth_account_event_store()
                .persist_and_transform(&mut transaction, created.clone())
                .await
                .unwrap();

            let all_events = db
                .auth_account_event_store()
                .find_by_id(&mut transaction, &event_id, None)
                .await
                .unwrap();
            assert_eq!(all_events.len(), 1);

            let no_events = db
                .auth_account_event_store()
                .find_by_id(&mut transaction, &event_id, Some(&create_envelope.version))
                .await
                .unwrap();
            assert_eq!(no_events.len(), 0);
        }
    }

    mod persist {
        use super::*;

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn basic_creation() {
            let db = PostgresDatabase::new().await.unwrap();
            let mut transaction = db.begin_transaction().await.unwrap();
            let id = AuthAccountId::new(Uuid::now_v7());
            let created = create_auth_account_command(id.clone());
            db.auth_account_event_store()
                .persist(&mut transaction, &created)
                .await
                .unwrap();
            let events = db
                .auth_account_event_store()
                .find_by_id(&mut transaction, &EventId::from(id), None)
                .await
                .unwrap();
            assert_eq!(events.len(), 1);
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn persist_and_transform_test() {
            let db = PostgresDatabase::new().await.unwrap();
            let mut transaction = db.begin_transaction().await.unwrap();
            let id = AuthAccountId::new(Uuid::now_v7());
            let created = create_auth_account_command(id.clone());

            let event_envelope = db
                .auth_account_event_store()
                .persist_and_transform(&mut transaction, created.clone())
                .await
                .unwrap();

            assert_eq!(event_envelope.id, EventId::from(id.clone()));
            assert_eq!(&event_envelope.event, created.event());

            let events = db
                .auth_account_event_store()
                .find_by_id(&mut transaction, &EventId::from(id), None)
                .await
                .unwrap();
            assert_eq!(events.len(), 1);
            assert_eq!(&events[0].event, created.event());
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn optimistic_concurrency_nothing() {
            let db = PostgresDatabase::new().await.unwrap();
            let mut transaction = db.begin_transaction().await.unwrap();
            let id = AuthAccountId::new(Uuid::now_v7());
            let created = create_auth_account_command(id.clone());
            db.auth_account_event_store()
                .persist(&mut transaction, &created)
                .await
                .unwrap();

            let duplicate = create_auth_account_command(id.clone());
            let result = db
                .auth_account_event_store()
                .persist(&mut transaction, &duplicate)
                .await;
            assert!(result.is_err());
        }
    }
}
