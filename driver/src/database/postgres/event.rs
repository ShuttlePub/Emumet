use crate::database::postgres::{CountRow, VersionRow};
use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use error_stack::Report;
use kernel::interfaces::modify::{DependOnEventModifier, EventModifier};
use kernel::interfaces::query::{DependOnEventQuery, EventQuery};
use kernel::prelude::entity::{
    CommandEnvelope, EventEnvelope, EventId, EventVersion, KnownEventVersion,
};
use kernel::KernelError;
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct EventRow {
    version: Uuid,
    id: Uuid,
    event_name: String,
    data: serde_json::Value,
}

impl<Event: for<'a> Deserialize<'a>, Entity> TryFrom<EventRow> for EventEnvelope<Event, Entity> {
    type Error = Report<KernelError>;
    fn try_from(value: EventRow) -> Result<Self, Self::Error> {
        let event: Event = serde_json::from_value(value.data).convert_error()?;
        Ok(EventEnvelope::new(
            EventId::new(value.id),
            event,
            EventVersion::new(value.version),
        ))
    }
}

pub struct PostgresEventRepository;

impl EventQuery for PostgresEventRepository {
    type Transaction = PostgresConnection;

    async fn find_by_id<Event: for<'de> Deserialize<'de>, Entity>(
        &self,
        transaction: &mut Self::Transaction,
        id: &EventId<Event, Entity>,
        since: Option<&EventVersion<Entity>>,
    ) -> error_stack::Result<Vec<EventEnvelope<Event, Entity>>, KernelError> {
        let con: &mut PgConnection = transaction;
        if let Some(version) = since {
            sqlx::query_as::<_, EventRow>(
                //language=postgresql
                r#"
                SELECT version, id, event_name, data
                FROM event_streams
                WHERE id = $2 AND version > $1
                ORDER BY version
                "#,
            )
            .bind(version.as_ref())
        } else {
            sqlx::query_as::<_, EventRow>(
                //language=postgresql
                r#"
                SELECT version, id, event_name, data
                FROM event_streams
                WHERE id = $1
                ORDER BY version
                "#,
            )
        }
        .bind(id.as_ref())
        .fetch_all(con)
        .await
        .convert_error()
        .and_then(|versions| {
            versions
                .into_iter()
                .map(|row| row.try_into())
                .collect::<error_stack::Result<Vec<EventEnvelope<Event, Entity>>, KernelError>>()
        })
    }
}

impl DependOnEventQuery for PostgresDatabase {
    type EventQuery = PostgresEventRepository;

    fn event_query(&self) -> &Self::EventQuery {
        &PostgresEventRepository
    }
}

impl EventModifier for PostgresEventRepository {
    type Transaction = PostgresConnection;

    async fn handle<Event: Serialize, Entity>(
        &self,
        transaction: &mut Self::Transaction,
        event: &CommandEnvelope<Event, Entity>,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = transaction;

        let event_name = event.event_name();
        let version = event.prev_version().as_ref();
        if let Some(prev_version) = version {
            match prev_version {
                KnownEventVersion::Nothing => {
                    let amount = sqlx::query_as::<_, CountRow>(
                        //language=postgresql
                        r#"
                    SELECT COUNT(*)
                    FROM event_streams
                    WHERE id = $1
                    "#,
                    )
                    .bind(event.id().as_ref())
                    .fetch_one(&mut *con)
                    .await
                    .convert_error()?;
                    if amount.count != 0 {
                        return Err(Report::new(KernelError::Concurrency).attach_printable(
                            format!("Event {} already exists", event.id().as_ref()),
                        ));
                    }
                }
                KnownEventVersion::Prev(prev_version) => {
                    let last_version = sqlx::query_as::<_, VersionRow>(
                        //language=postgresql
                        r#"
                        SELECT version
                        FROM event_streams
                        WHERE id = $1
                        ORDER BY version DESC
                        LIMIT 1
                        "#,
                    )
                    .bind(event.id().as_ref())
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
                                event.id().as_ref(),
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
                INSERT INTO event_streams (version, id, event_name, data)
                VALUES ($1, $2, $3, $4)
                "#,
        )
        .bind(Uuid::now_v7())
        .bind(event.id().as_ref())
        .bind(event_name)
        .bind(serde_json::to_value(event.event()).convert_error()?)
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }
}

impl DependOnEventModifier for PostgresDatabase {
    type EventModifier = PostgresEventRepository;

    fn event_modifier(&self) -> &Self::EventModifier {
        &PostgresEventRepository
    }
}

#[cfg(test)]
mod test {
    mod query {
        use uuid::Uuid;

        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::modify::{DependOnEventModifier, EventModifier};
        use kernel::interfaces::query::{DependOnEventQuery, EventQuery};
        use kernel::prelude::entity::{
            Account, AccountId, AccountIsBot, AccountName, AccountPrivateKey, AccountPublicKey,
            EventId, Nanoid,
        };

        use crate::database::PostgresDatabase;

        #[tokio::test]
        async fn find_by_id() {
            let db = PostgresDatabase::new().await.unwrap();
            let mut transaction = db.begin_transaction().await.unwrap();
            let account_id = AccountId::new(Uuid::now_v7());
            let event_id = EventId::from(account_id.clone());
            let events = db
                .event_query()
                .find_by_id(&mut transaction, &event_id, None)
                .await
                .unwrap();
            assert_eq!(events.len(), 0);
            let created_account = Account::create(
                account_id.clone(),
                AccountName::new("test"),
                AccountPrivateKey::new("test"),
                AccountPublicKey::new("test"),
                AccountIsBot::new(false),
                Nanoid::default(),
            );
            let updated_account = Account::update(account_id.clone(), AccountIsBot::new(true));
            let deleted_account = Account::delete(account_id.clone());

            db.event_modifier()
                .handle(&mut transaction, &created_account)
                .await
                .unwrap();
            db.event_modifier()
                .handle(&mut transaction, &updated_account)
                .await
                .unwrap();
            db.event_modifier()
                .handle(&mut transaction, &deleted_account)
                .await
                .unwrap();
            let events = db
                .event_query()
                .find_by_id(&mut transaction, &event_id, None)
                .await
                .unwrap();
            assert_eq!(events.len(), 3);
            assert_eq!(&events[0].event, created_account.event());
            assert_eq!(&events[1].event, updated_account.event());
            assert_eq!(&events[2].event, deleted_account.event());
        }

        #[tokio::test]
        #[should_panic]
        async fn find_by_id_with_version() {
            let db = PostgresDatabase::new().await.unwrap();
            let mut transaction = db.begin_transaction().await.unwrap();
            let account_id = AccountId::new(Uuid::now_v7());
            let event_id = EventId::from(account_id.clone());
            let created_account = Account::create(
                account_id.clone(),
                AccountName::new("test"),
                AccountPrivateKey::new("test"),
                AccountPublicKey::new("test"),
                AccountIsBot::new(false),
                Nanoid::default(),
            );
            let updated_account = Account::update(account_id.clone(), AccountIsBot::new(true));
            db.event_modifier()
                .handle(&mut transaction, &created_account)
                .await
                .unwrap();
            db.event_modifier()
                .handle(&mut transaction, &updated_account)
                .await
                .unwrap();

            let all_events = db
                .event_query()
                .find_by_id(&mut transaction, &event_id, None)
                .await
                .unwrap();
            let events = db
                .event_query()
                .find_by_id(&mut transaction, &event_id, Some(&all_events[1].version))
                .await
                .unwrap();
            assert_eq!(events.len(), 1);
            let event = &events[0];
            assert_eq!(&event.event, updated_account.event());
        }
    }

    mod modify {
        use uuid::Uuid;

        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::modify::{DependOnEventModifier, EventModifier};
        use kernel::interfaces::query::{DependOnEventQuery, EventQuery};
        use kernel::prelude::entity::{
            Account, AccountId, AccountIsBot, AccountName, AccountPrivateKey, AccountPublicKey,
            EventId, Nanoid,
        };

        use crate::database::PostgresDatabase;

        #[tokio::test]
        async fn basic_creation() {
            let db = PostgresDatabase::new().await.unwrap();
            let mut transaction = db.begin_transaction().await.unwrap();
            let account_id = AccountId::new(Uuid::now_v7());
            let created_account = Account::create(
                account_id.clone(),
                AccountName::new("test"),
                AccountPrivateKey::new("test"),
                AccountPublicKey::new("test"),
                AccountIsBot::new(false),
                Nanoid::default(),
            );
            db.event_modifier()
                .handle(&mut transaction, &created_account)
                .await
                .unwrap();
            let events = db
                .event_query()
                .find_by_id(&mut transaction, &EventId::from(account_id), None)
                .await
                .unwrap();
            assert_eq!(events.len(), 1);
        }
    }
}
