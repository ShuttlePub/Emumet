use error_stack::Report;
use sqlx::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;

use kernel::interfaces::modify::{AccountEventModifier, DependOnAccountEventModifier};
use kernel::interfaces::query::{AccountEventQuery, DependOnAccountEventQuery};
use kernel::prelude::entity::{
    Account, AccountEvent, AccountId, CommandEnvelope, CreatedAt, EventEnvelope, EventVersion,
    ExpectedEventVersion,
};
use kernel::KernelError;

use crate::database::postgres::{CountRow, VersionRow};
use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;

#[derive(sqlx::FromRow)]
struct AccountEventRow {
    version: i64,
    account_id: Uuid,
    event_name: String,
    data: serde_json::Value,
    created_at: OffsetDateTime,
}

impl TryFrom<AccountEventRow> for EventEnvelope<AccountEvent, Account> {
    type Error = Report<KernelError>;
    fn try_from(row: AccountEventRow) -> Result<Self, Self::Error> {
        let event: AccountEvent = serde_json::from_value(row.data).convert_error()?;
        Ok(EventEnvelope::new(
            event,
            EventVersion::new(row.version),
            CreatedAt::new(row.created_at),
        ))
    }
}

pub struct PostgresAccountEventRepository;

impl AccountEventQuery for PostgresAccountEventRepository {
    type Transaction = PostgresConnection;

    async fn find_by_id(
        &self,
        transaction: &mut Self::Transaction,
        id: &AccountId,
        since: Option<&EventVersion<Account>>,
    ) -> error_stack::Result<Vec<EventEnvelope<AccountEvent, Account>>, KernelError> {
        let con: &mut PgConnection = transaction;
        if let Some(version) = since {
            sqlx::query_as::<_, AccountEventRow>(
                //language=postgresql
                r#"
                SELECT version, account_id, event_name, data, created_at
                FROM account_events
                WHERE account_id = $2 AND version > $1
                ORDER BY version ASC
                "#,
            )
            .bind(version.as_ref())
        } else {
            sqlx::query_as::<_, AccountEventRow>(
                //language=postgresql
                r#"
                SELECT version, account_id, event_name, data, created_at
                FROM account_events
                WHERE account_id = $1
                ORDER BY version ASC
                "#,
            )
        }
        .bind(id.as_ref())
        .fetch_all(con)
        .await
        .convert_error()
        .and_then(|versions| versions.into_iter().map(|row| row.try_into()).collect())
    }
}

impl DependOnAccountEventQuery for PostgresDatabase {
    type AccountEventQuery = PostgresAccountEventRepository;

    fn account_event_query(&self) -> &Self::AccountEventQuery {
        &PostgresAccountEventRepository
    }
}

impl AccountEventModifier for PostgresAccountEventRepository {
    type Transaction = PostgresConnection;

    async fn handle(
        &self,
        transaction: &mut Self::Transaction,
        account_id: &AccountId,
        event: &CommandEnvelope<AccountEvent, Account>,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = transaction;

        let event_name = event.event().name();
        let version = event.version().as_ref();
        if let Some(version) = version {
            let version: i64 = match version {
                ExpectedEventVersion::Nothing => {
                    let amount = sqlx::query_as::<_, CountRow>(
                        //language=postgresql
                        r#"
                    SELECT COUNT(*)
                    FROM account_events
                    WHERE account_id = $1
                    "#,
                    )
                    .bind(account_id.as_ref())
                    .fetch_one(&mut *con)
                    .await
                    .convert_error()?;
                    if amount.count != 0 {
                        return Err(Report::new(KernelError::Concurrency).attach_printable(
                            format!("Account {} already exists", account_id.as_ref()),
                        ));
                    }
                    0
                }
                ExpectedEventVersion::Exact(version) => {
                    let last_version = sqlx::query_as::<_, VersionRow>(
                        //language=postgresql
                        r#"
                        SELECT version
                        FROM account_events
                        WHERE account_id = $1
                        ORDER BY version DESC
                        LIMIT 1
                        "#,
                    )
                    .bind(account_id.as_ref())
                    .fetch_optional(&mut *con)
                    .await
                    .convert_error()?;
                    if last_version
                        .map(|row: VersionRow| row.version != *version.as_ref())
                        .unwrap_or(true)
                    {
                        return Err(Report::new(KernelError::Concurrency).attach_printable(
                            format!(
                                "Account {} version {} already exists",
                                account_id.as_ref(),
                                version.as_ref()
                            ),
                        ));
                    }
                    *version.as_ref()
                }
            };
            sqlx::query(
                //language=postgresql
                r#"
                INSERT INTO account_events (version, account_id, event_name, data, created_at)
                VALUES ($1, $2, $3, $4, now())
                "#,
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
                //language=postgresql
                r#"
                INSERT INTO account_events (account_id, event_name, data, created_at)
                VALUES ($1, $2, $3, now())
                "#,
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

impl DependOnAccountEventModifier for PostgresDatabase {
    type AccountEventModifier = PostgresAccountEventRepository;

    fn account_event_modifier(&self) -> &Self::AccountEventModifier {
        &PostgresAccountEventRepository
    }
}

impl PostgresAccountEventRepository {
    // Used in the test
    async fn delete(
        &self,
        transaction: &mut PostgresConnection,
        account_id: &AccountId,
        event: &EventVersion<Account>,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = transaction;
        sqlx::query(
            //language=postgresql
            r#"
            DELETE FROM account_events
            WHERE account_id = $1 AND version = $2
            "#,
        )
        .bind(account_id.as_ref())
        .bind(event.as_ref())
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }
}

#[cfg(test)]
mod test {
    mod query {
        use uuid::Uuid;

        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::modify::{AccountEventModifier, DependOnAccountEventModifier};
        use kernel::interfaces::query::{AccountEventQuery, DependOnAccountEventQuery};
        use kernel::prelude::entity::{
            Account, AccountId, AccountIsBot, AccountName, AccountPrivateKey, AccountPublicKey,
        };

        use crate::database::PostgresDatabase;

        #[tokio::test]
        async fn find_by_id() {
            let db = PostgresDatabase::new().await.unwrap();
            let mut transaction = db.begin_transaction().await.unwrap();
            let account_id = AccountId::new(Uuid::new_v4());
            let events = db
                .account_event_query()
                .find_by_id(&mut transaction, &account_id, None)
                .await
                .unwrap();
            assert_eq!(events.len(), 0);
            let created_account = Account::create(
                AccountName::new("test"),
                AccountPrivateKey::new("test"),
                AccountPublicKey::new("test"),
                AccountIsBot::new(false),
            );
            let updated_account = Account::update(AccountIsBot::new(true));
            let deleted_account = Account::delete();

            db.account_event_modifier()
                .handle(&mut transaction, &account_id, &deleted_account)
                .await
                .unwrap();
            db.account_event_modifier()
                .handle(&mut transaction, &account_id, &updated_account)
                .await
                .unwrap();
            db.account_event_modifier()
                .handle(&mut transaction, &account_id, &created_account)
                .await
                .unwrap();
            let events = db
                .account_event_query()
                .find_by_id(&mut transaction, &account_id, None)
                .await
                .unwrap();
            assert_eq!(events.len(), 3);
            assert_eq!(events[0].event(), created_account.event());
            assert_eq!(events[1].event(), updated_account.event());
            assert_eq!(events[2].event(), deleted_account.event());
        }

        #[tokio::test]
        #[should_panic]
        async fn find_by_id_with_version() {
            let db = PostgresDatabase::new().await.unwrap();
            let mut transaction = db.begin_transaction().await.unwrap();
            let account_id = AccountId::new(Uuid::new_v4());
            let created_account = Account::create(
                AccountName::new("test"),
                AccountPrivateKey::new("test"),
                AccountPublicKey::new("test"),
                AccountIsBot::new(false),
            );
            let updated_account = Account::update(AccountIsBot::new(true));
            db.account_event_modifier()
                .handle(&mut transaction, &account_id, &created_account)
                .await
                .unwrap();
            db.account_event_modifier()
                .handle(&mut transaction, &account_id, &updated_account)
                .await
                .unwrap();

            let all_events = db
                .account_event_query()
                .find_by_id(&mut transaction, &account_id, None)
                .await
                .unwrap();
            let events = db
                .account_event_query()
                .find_by_id(&mut transaction, &account_id, Some(all_events[1].version()))
                .await
                .unwrap();
            assert_eq!(events.len(), 1);
            let event = &events[0];
            assert_eq!(event.event(), &updated_account.event());
        }
    }

    mod modify {
        use uuid::Uuid;

        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::modify::{AccountEventModifier, DependOnAccountEventModifier};
        use kernel::interfaces::query::{AccountEventQuery, DependOnAccountEventQuery};
        use kernel::prelude::entity::{
            Account, AccountId, AccountIsBot, AccountName, AccountPrivateKey, AccountPublicKey,
            EventVersion,
        };

        use crate::database::PostgresDatabase;

        #[tokio::test]
        async fn basic_creation() {
            let db = PostgresDatabase::new().await.unwrap();
            let mut transaction = db.begin_transaction().await.unwrap();
            let account_id = AccountId::new(Uuid::new_v4());
            let created_account = Account::create(
                AccountName::new("test"),
                AccountPrivateKey::new("test"),
                AccountPublicKey::new("test"),
                AccountIsBot::new(false),
            );
            db.account_event_modifier()
                .handle(&mut transaction, &account_id, &created_account)
                .await
                .unwrap();
            let events = db
                .account_event_query()
                .find_by_id(&mut transaction, &account_id, None)
                .await
                .unwrap();
            assert_eq!(events.len(), 1);
            let event = &events[0];
            assert_eq!(event.version().as_ref(), &EventVersion::new(1));
        }
    }
}
