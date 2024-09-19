use crate::database::postgres::{CountRow, VersionRow};
use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use error_stack::{Report, ResultExt};
use kernel::interfaces::modify::{
    DependOnStellarAccountEventModifier, StellarAccountEventModifier,
};
use kernel::interfaces::query::{DependOnStellarAccountEventQuery, StellarAccountEventQuery};
use kernel::prelude::entity::{
    CommandEnvelope, CreatedAt, EventEnvelope, EventVersion, ExpectedEventVersion, StellarAccount,
    StellarAccountEvent, StellarAccountId,
};
use kernel::KernelError;
use sqlx::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct StellarAccountEventRow {
    version: i64,
    stellar_account_id: Uuid,
    event_name: String,
    data: serde_json::Value,
    created_at: OffsetDateTime,
}

impl TryFrom<StellarAccountEventRow> for EventEnvelope<StellarAccountEvent, StellarAccount> {
    type Error = Report<KernelError>;
    fn try_from(value: StellarAccountEventRow) -> Result<Self, Self::Error> {
        let event: StellarAccountEvent =
            serde_json::from_value(value.data.clone()).convert_error()?;
        Ok(EventEnvelope::new(
            event,
            EventVersion::new(value.version),
            CreatedAt::new(value.created_at),
        ))
    }
}

pub struct PostgresStellarAccountEventRepository;

impl StellarAccountEventQuery for PostgresStellarAccountEventRepository {
    type Transaction = PostgresConnection;

    async fn find_by_id(
        &self,
        transaction: &mut Self::Transaction,
        id: &StellarAccountId,
        since: Option<&EventVersion<StellarAccount>>,
    ) -> error_stack::Result<Vec<EventEnvelope<StellarAccountEvent, StellarAccount>>, KernelError>
    {
        let mut con: &mut PgConnection = transaction;
        if let Some(version) = since {
            sqlx::query_as::<_, StellarAccountEventRow>(
                // language=postgresql
                r#"
                SELECT version, stellar_account_id, event_name, data, created_at
                FROM stellar_account_events
                WHERE stellar_account_id = $1 AND version > $2
                ORDER BY version
                "#,
            )
            .bind(version.as_ref())
        } else {
            sqlx::query_as::<_, StellarAccountEventRow>(
                // language=postgresql
                r#"
                SELECT version, stellar_account_id, event_name, data, created_at
                FROM stellar_account_events
                WHERE stellar_account_id = $1
                ORDER BY version
                "#,
            )
        }
        .bind(id.as_ref())
        .fetch_all(con)
        .await
        .convert_error()
        .and_then(|rows| rows.into_iter().map(|row| row.try_into()).collect())
    }
}

impl DependOnStellarAccountEventQuery for PostgresDatabase {
    type StellarAccountEventQuery = PostgresStellarAccountEventRepository;

    fn stellar_account_event_query(&self) -> &Self::StellarAccountEventQuery {
        &PostgresStellarAccountEventRepository
    }
}

impl StellarAccountEventModifier for PostgresStellarAccountEventRepository {
    type Transaction = PostgresConnection;

    async fn handle(
        &self,
        transaction: &mut Self::Transaction,
        account_id: &StellarAccountId,
        event: &CommandEnvelope<StellarAccountEvent, StellarAccount>,
    ) -> error_stack::Result<(), KernelError> {
        let mut con: &mut PgConnection = transaction;
        let event_name = event.event().name();
        let version = event.version().as_ref();
        if let Some(version) = version {
            let version = match version {
                ExpectedEventVersion::Nothing => {
                    let amount = sqlx::query_as::<_, CountRow>(
                        // language=postgresql
                        r#"
                        SELECT COUNT(*)
                        FROM stellar_account_events
                        WHERE stellar_account_id = $1
                        "#,
                    )
                    .bind(account_id.as_ref())
                    .fetch_one(&mut *con)
                    .await
                    .convert_error()?;
                    if amount.count != 0 {
                        return Err(KernelError::Concurrency).attach_printable(format!(
                            "The account {} already has events",
                            account_id.as_ref()
                        ));
                    }
                    0
                }
                ExpectedEventVersion::Exact(version) => {
                    let last_version = sqlx::query_as::<_, VersionRow>(
                        // language=postgresql
                        r#"
                        SELECT version
                        FROM stellar_account_events
                        WHERE stellar_account_id = $1
                        ORDER BY version DESC
                        LIMIT 1
                        "#,
                    )
                    .bind(account_id.as_ref())
                    .fetch_one(&mut *con)
                    .await
                    .convert_error()?;
                    if last_version.version != *version.as_ref() {
                        return Err(KernelError::Concurrency).attach_printable(format!(
                            "The account {} has version {} already exists",
                            account_id.as_ref(),
                            version.as_ref()
                        ));
                    }
                    *version.as_ref() + 1
                }
            };
            sqlx::query(
                // language=postgresql
                r#"
                INSERT INTO stellar_account_events (version, stellar_account_id, event_name, data, created_at)
                VALUES ($1, $2, $3, $4, now())
                "#
            )
                .bind(version)
                .bind(account_id.as_ref())
                .bind(event_name)
                .bind(serde_json::to_value(event.event()).convert_error()?)
                .execute(con)
                .await
                .convert_error()?;
        } else {
            sqlx::query(
                // language=postgresql
                r#"
                INSERT INTO stellar_account_events (stellar_account_id, event_name, data, created_at)
                VALUES ($1, $2, $3, now())
                "#
            )
                .bind(account_id.as_ref())
                .bind(event_name)
                .bind(serde_json::to_value(event.event()).convert_error()?)
                .execute(con)
                .await
                .convert_error()?;
        }
        Ok(())
    }
}

impl DependOnStellarAccountEventModifier for PostgresDatabase {
    type StellarAccountEventModifier = PostgresStellarAccountEventRepository;

    fn stellar_account_event_modifier(&self) -> &Self::StellarAccountEventModifier {
        &PostgresStellarAccountEventRepository
    }
}

#[cfg(test)]
mod test {
    mod query {
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::modify::{
            DependOnStellarAccountEventModifier, StellarAccountEventModifier,
        };
        use kernel::interfaces::query::{
            DependOnStellarAccountEventQuery, StellarAccountEventQuery,
        };
        use kernel::prelude::entity::{
            EventEnvelope, EventVersion, StellarAccount, StellarAccountAccessToken,
            StellarAccountClientId, StellarAccountEvent, StellarAccountHost, StellarAccountId,
            StellarAccountRefreshToken,
        };
        use uuid::Uuid;

        #[tokio::test]
        async fn find_by_id() {
            let db = PostgresDatabase::new().await.unwrap();
            let mut con = db.begin_transaction().await.unwrap();

            let stellar_account_id = StellarAccountId::new(Uuid::new_v4());

            let events = db
                .stellar_account_event_query()
                .find_by_id(&mut con, &stellar_account_id, None)
                .await
                .unwrap();
            assert_eq!(events.len(), 0);

            let created_stellar_account = StellarAccount::create(
                StellarAccountHost::new("host".to_string()),
                StellarAccountClientId::new("client_id".to_string()),
                StellarAccountAccessToken::new("access_token".to_string()),
                StellarAccountRefreshToken::new("refresh_token".to_string()),
            );
            let updated_stellar_account = StellarAccount::update(
                StellarAccountAccessToken::new("access_token".to_string()),
                StellarAccountRefreshToken::new("refresh_token".to_string()),
            );
            let deleted_stellar_account = StellarAccount::delete();

            db.stellar_account_event_modifier()
                .handle(&mut con, &stellar_account_id, &created_stellar_account)
                .await
                .unwrap();
            db.stellar_account_event_modifier()
                .handle(&mut con, &stellar_account_id, &updated_stellar_account)
                .await
                .unwrap();
            db.stellar_account_event_modifier()
                .handle(&mut con, &stellar_account_id, &deleted_stellar_account)
                .await
                .unwrap();

            let events = db
                .stellar_account_event_query()
                .find_by_id(&mut con, &stellar_account_id, None)
                .await
                .unwrap();
            assert_eq!(events.len(), 3);
            assert_eq!(events[0].event(), &created_stellar_account);
            assert_eq!(events[1].event(), &updated_stellar_account);
            assert_eq!(events[2].event(), &deleted_stellar_account);
        }

        #[tokio::test]
        async fn find_by_id_since() {
            let db = PostgresDatabase::new().await.unwrap();
            let mut con = db.begin_transaction().await.unwrap();

            let stellar_account_id = StellarAccountId::new(Uuid::new_v4());

            let events = db
                .stellar_account_event_query()
                .find_by_id(&mut con, &stellar_account_id, None)
                .await
                .unwrap();
            assert_eq!(events.len(), 0);

            let created_stellar_account = StellarAccount::create(
                StellarAccountHost::new("host".to_string()),
                StellarAccountClientId::new("client_id".to_string()),
                StellarAccountAccessToken::new("access_token".to_string()),
                StellarAccountRefreshToken::new("refresh_token".to_string()),
            );
            let updated_stellar_account = StellarAccount::update(
                StellarAccountAccessToken::new("access_token".to_string()),
                StellarAccountRefreshToken::new("refresh_token".to_string()),
            );
            let deleted_stellar_account = StellarAccount::delete();

            db.stellar_account_event_modifier()
                .handle(&mut con, &stellar_account_id, &created_stellar_account)
                .await
                .unwrap();
            db.stellar_account_event_modifier()
                .handle(&mut con, &stellar_account_id, &updated_stellar_account)
                .await
                .unwrap();
            db.stellar_account_event_modifier()
                .handle(&mut con, &stellar_account_id, &deleted_stellar_account)
                .await
                .unwrap();

            let events = db
                .stellar_account_event_query()
                .find_by_id(&mut con, &stellar_account_id, Some(&EventVersion::new(1)))
                .await
                .unwrap();
            assert_eq!(events.len(), 2);
            assert_eq!(events[0].event(), &updated_stellar_account);
            assert_eq!(events[1].event(), &deleted_stellar_account);
        }
    }

    mod modify {
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::modify::{
            DependOnStellarAccountEventModifier, StellarAccountEventModifier,
        };
        use kernel::interfaces::query::{
            DependOnStellarAccountEventQuery, StellarAccountEventQuery,
        };
        use kernel::prelude::entity::{
            StellarAccount, StellarAccountAccessToken, StellarAccountClientId, StellarAccountHost,
            StellarAccountId, StellarAccountRefreshToken,
        };

        #[tokio::test]
        async fn handle() {
            let db = PostgresDatabase::new().await.unwrap();
            let mut con = db.begin_transaction().await.unwrap();

            let stellar_account_id = StellarAccountId::new(uuid::Uuid::new_v4());

            let created_stellar_account = StellarAccount::create(
                StellarAccountHost::new("host".to_string()),
                StellarAccountClientId::new("client_id".to_string()),
                StellarAccountAccessToken::new("access_token".to_string()),
                StellarAccountRefreshToken::new("refresh_token".to_string()),
            );
            let updated_stellar_account = StellarAccount::update(
                StellarAccountAccessToken::new("access_token".to_string()),
                StellarAccountRefreshToken::new("refresh_token".to_string()),
            );
            let deleted_stellar_account = StellarAccount::delete();

            db.stellar_account_event_modifier()
                .handle(&mut con, &stellar_account_id, &created_stellar_account)
                .await
                .unwrap();
            db.stellar_account_event_modifier()
                .handle(&mut con, &stellar_account_id, &updated_stellar_account)
                .await
                .unwrap();
            db.stellar_account_event_modifier()
                .handle(&mut con, &stellar_account_id, &deleted_stellar_account)
                .await
                .unwrap();

            let events = db
                .stellar_account_event_query()
                .find_by_id(&mut con, &stellar_account_id, None)
                .await
                .unwrap();
            assert_eq!(events.len(), 3);
            assert_eq!(events[0].event(), &created_stellar_account);
            assert_eq!(events[1].event(), &updated_stellar_account);
            assert_eq!(events[2].event(), &deleted_stellar_account);
        }
    }
}
