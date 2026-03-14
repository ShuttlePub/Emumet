use crate::database::postgres::{CountRow, VersionRow};
use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use error_stack::Report;
use kernel::interfaces::event_store::{AccountEventStore, DependOnAccountEventStore};
use kernel::prelude::entity::{
    Account, AccountEvent, CommandEnvelope, EventEnvelope, EventId, EventVersion, KnownEventVersion,
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

impl TryFrom<EventRow> for EventEnvelope<AccountEvent, Account> {
    type Error = Report<KernelError>;
    fn try_from(value: EventRow) -> Result<Self, Self::Error> {
        let event: AccountEvent = serde_json::from_value(value.data).convert_error()?;
        Ok(EventEnvelope::new(
            EventId::new(value.id),
            event,
            EventVersion::new(value.version),
        ))
    }
}

pub struct PostgresAccountEventStore;

impl AccountEventStore for PostgresAccountEventStore {
    type Executor = PostgresConnection;

    async fn find_by_id(
        &self,
        executor: &mut Self::Executor,
        id: &EventId<AccountEvent, Account>,
        since: Option<&EventVersion<Account>>,
    ) -> error_stack::Result<Vec<EventEnvelope<AccountEvent, Account>>, KernelError> {
        let con: &mut PgConnection = executor;
        let rows = if let Some(version) = since {
            sqlx::query_as::<_, EventRow>(
                //language=postgresql
                r#"
                SELECT version, id, event_name, data
                FROM account_events
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
                FROM account_events
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
        command: &CommandEnvelope<AccountEvent, Account>,
    ) -> error_stack::Result<(), KernelError> {
        self.persist_internal(executor, command, kernel::generate_id())
            .await
    }

    async fn persist_and_transform(
        &self,
        executor: &mut Self::Executor,
        command: CommandEnvelope<AccountEvent, Account>,
    ) -> error_stack::Result<EventEnvelope<AccountEvent, Account>, KernelError> {
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

impl PostgresAccountEventStore {
    async fn persist_internal(
        &self,
        executor: &mut PostgresConnection,
        command: &CommandEnvelope<AccountEvent, Account>,
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
                        FROM account_events
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
                        FROM account_events
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
            INSERT INTO account_events (version, id, event_name, data)
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

impl DependOnAccountEventStore for PostgresDatabase {
    type AccountEventStore = PostgresAccountEventStore;

    fn account_event_store(&self) -> &Self::AccountEventStore {
        &PostgresAccountEventStore
    }
}

#[cfg(test)]
mod test {
    mod query {
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::event_store::{AccountEventStore, DependOnAccountEventStore};
        use kernel::prelude::entity::{
            Account, AccountEvent, AccountId, AccountIsBot, AccountName, AccountPrivateKey,
            AccountPublicKey, AuthAccountId, CommandEnvelope, EventId, KnownEventVersion, Nanoid,
        };

        fn create_account_command(account_id: AccountId) -> CommandEnvelope<AccountEvent, Account> {
            let event = AccountEvent::Created {
                name: AccountName::new("test"),
                private_key: AccountPrivateKey::new("test"),
                public_key: AccountPublicKey::new("test"),
                is_bot: AccountIsBot::new(false),
                nanoid: Nanoid::default(),
                auth_account_id: AuthAccountId::default(),
            };
            CommandEnvelope::new(
                EventId::from(account_id),
                event.name(),
                event,
                Some(KnownEventVersion::Nothing),
            )
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_id() {
            kernel::ensure_generator_initialized();
            let db = PostgresDatabase::new().await.unwrap();
            let mut transaction = db.begin_transaction().await.unwrap();
            let account_id = AccountId::default();
            let event_id = EventId::from(account_id.clone());
            let events = db
                .account_event_store()
                .find_by_id(&mut transaction, &event_id, None)
                .await
                .unwrap();
            assert_eq!(events.len(), 0);
            let created_account = create_account_command(account_id.clone());
            let update_event = AccountEvent::Updated {
                is_bot: AccountIsBot::new(true),
            };
            let updated_account = CommandEnvelope::new(
                EventId::from(account_id.clone()),
                update_event.name(),
                update_event,
                None,
            );
            let delete_event = AccountEvent::Deactivated;
            let deleted_account = CommandEnvelope::new(
                EventId::from(account_id.clone()),
                delete_event.name(),
                delete_event,
                None,
            );

            db.account_event_store()
                .persist(&mut transaction, &created_account)
                .await
                .unwrap();
            db.account_event_store()
                .persist(&mut transaction, &updated_account)
                .await
                .unwrap();
            db.account_event_store()
                .persist(&mut transaction, &deleted_account)
                .await
                .unwrap();
            let events = db
                .account_event_store()
                .find_by_id(&mut transaction, &event_id, None)
                .await
                .unwrap();
            assert_eq!(events.len(), 3);
            assert_eq!(&events[0].event, created_account.event());
            assert_eq!(&events[1].event, updated_account.event());
            assert_eq!(&events[2].event, deleted_account.event());
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_id_since_version() {
            kernel::ensure_generator_initialized();
            let db = PostgresDatabase::new().await.unwrap();
            let mut transaction = db.begin_transaction().await.unwrap();
            let account_id = AccountId::default();
            let event_id = EventId::from(account_id.clone());

            let created_account = create_account_command(account_id.clone());
            let update_event = AccountEvent::Updated {
                is_bot: AccountIsBot::new(true),
            };
            let updated_account = CommandEnvelope::new(
                EventId::from(account_id.clone()),
                update_event.name(),
                update_event,
                None,
            );
            let delete_event = AccountEvent::Deactivated;
            let deleted_account = CommandEnvelope::new(
                EventId::from(account_id.clone()),
                delete_event.name(),
                delete_event,
                None,
            );

            db.account_event_store()
                .persist(&mut transaction, &created_account)
                .await
                .unwrap();
            db.account_event_store()
                .persist(&mut transaction, &updated_account)
                .await
                .unwrap();
            db.account_event_store()
                .persist(&mut transaction, &deleted_account)
                .await
                .unwrap();

            // Get all events to obtain the first version
            let all_events = db
                .account_event_store()
                .find_by_id(&mut transaction, &event_id, None)
                .await
                .unwrap();
            assert_eq!(all_events.len(), 3);

            // Query since the first event's version — should return the 2nd and 3rd events
            let since_events = db
                .account_event_store()
                .find_by_id(&mut transaction, &event_id, Some(&all_events[0].version))
                .await
                .unwrap();
            assert_eq!(since_events.len(), 2);
            assert_eq!(&since_events[0].event, updated_account.event());
            assert_eq!(&since_events[1].event, deleted_account.event());

            // Query since the last event's version — should return no events
            let no_events = db
                .account_event_store()
                .find_by_id(&mut transaction, &event_id, Some(&all_events[2].version))
                .await
                .unwrap();
            assert_eq!(no_events.len(), 0);
        }
    }

    mod persist {
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::event_store::{AccountEventStore, DependOnAccountEventStore};
        use kernel::prelude::entity::{
            Account, AccountEvent, AccountId, AccountIsBot, AccountName, AccountPrivateKey,
            AccountPublicKey, AuthAccountId, CommandEnvelope, EventId, KnownEventVersion, Nanoid,
        };

        fn create_account_command(account_id: AccountId) -> CommandEnvelope<AccountEvent, Account> {
            let event = AccountEvent::Created {
                name: AccountName::new("test"),
                private_key: AccountPrivateKey::new("test"),
                public_key: AccountPublicKey::new("test"),
                is_bot: AccountIsBot::new(false),
                nanoid: Nanoid::default(),
                auth_account_id: AuthAccountId::default(),
            };
            CommandEnvelope::new(
                EventId::from(account_id),
                event.name(),
                event,
                Some(KnownEventVersion::Nothing),
            )
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn basic_creation() {
            kernel::ensure_generator_initialized();
            let db = PostgresDatabase::new().await.unwrap();
            let mut transaction = db.begin_transaction().await.unwrap();
            let account_id = AccountId::default();
            let created_account = create_account_command(account_id.clone());
            db.account_event_store()
                .persist(&mut transaction, &created_account)
                .await
                .unwrap();
            let events = db
                .account_event_store()
                .find_by_id(&mut transaction, &EventId::from(account_id), None)
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
            let account_id = AccountId::default();
            let created_account = create_account_command(account_id.clone());

            let event_envelope = db
                .account_event_store()
                .persist_and_transform(&mut transaction, created_account.clone())
                .await
                .unwrap();

            assert_eq!(event_envelope.id, EventId::from(account_id.clone()));
            assert_eq!(&event_envelope.event, created_account.event());

            let events = db
                .account_event_store()
                .find_by_id(&mut transaction, &EventId::from(account_id), None)
                .await
                .unwrap();
            assert_eq!(events.len(), 1);
            assert_eq!(&events[0].event, created_account.event());
        }
    }
}
