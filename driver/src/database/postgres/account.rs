use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use kernel::interfaces::modify::{AccountModifier, DependOnAccountModifier};
use kernel::interfaces::query::{AccountQuery, DependOnAccountQuery};
use kernel::prelude::entity::{
    Account, AccountId, AccountIsBot, AccountName, AccountPrivateKey, AccountPublicKey, CreatedAt,
    DeletedAt, StellarAccountId,
};
use kernel::KernelError;
use sqlx::types::time::OffsetDateTime;
use sqlx::types::Uuid;
use sqlx::PgConnection;

#[derive(sqlx::FromRow)]
struct AccountRow {
    id: Uuid,
    name: String,
    private_key: String,
    public_key: String,
    is_bot: bool,
    created_at: OffsetDateTime,
    deleted_at: Option<OffsetDateTime>,
}

impl From<AccountRow> for Account {
    fn from(value: AccountRow) -> Self {
        Account::new(
            AccountId::new(value.id),
            AccountName::new(value.name),
            AccountPrivateKey::new(value.private_key),
            AccountPublicKey::new(value.public_key),
            AccountIsBot::new(value.is_bot),
            CreatedAt::new(value.created_at),
            value.deleted_at.map(DeletedAt::new),
        )
    }
}

pub struct PostgresAccountRepository;

impl AccountQuery for PostgresAccountRepository {
    type Transaction = PostgresConnection;

    async fn find_by_id(
        &self,
        transaction: &mut Self::Transaction,
        id: &AccountId,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        let con: &mut PgConnection = transaction;
        sqlx::query_as::<_, AccountRow>(
            //language=postgresql
            r#"
            SELECT id, name, private_key, public_key, is_bot, created_at, deleted_at
            FROM accounts
            WHERE id = $1
            "#,
        )
        .bind(id.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()
        .map(|option| option.map(Account::from))
    }

    async fn find_by_stellar_id(
        &self,
        transaction: &mut Self::Transaction,
        stellar_id: &StellarAccountId,
    ) -> error_stack::Result<Vec<Account>, KernelError> {
        let con: &mut PgConnection = transaction;
        sqlx::query_as::<_, AccountRow>(
            //language=postgresql
            r#"
            SELECT id, name, private_key, public_key, is_bot, created_at, deleted_at
            FROM accounts
            INNER JOIN stellar_emumet_accounts ON stellar_emumet_accounts.emumet_id = accounts.id
            WHERE stellar_emumet_accounts.stellar_id = $1
            "#,
        )
        .bind(stellar_id.as_ref())
        .fetch_all(con)
        .await
        .convert_error()
        .map(|rows| rows.into_iter().map(Account::from).collect())
    }

    async fn find_by_name(
        &self,
        transaction: &mut Self::Transaction,
        name: &AccountName,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        let con: &mut PgConnection = transaction;
        sqlx::query_as::<_, AccountRow>(
            //language=postgresql
            r#"
            SELECT id, name, private_key, public_key, is_bot, created_at, deleted_at
            FROM accounts
            WHERE name = $1
            "#,
        )
        .bind(name.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()
        .map(|option| option.map(Account::from))
    }
}

impl DependOnAccountQuery for PostgresDatabase {
    type AccountQuery = PostgresAccountRepository;

    fn account_query(&self) -> &Self::AccountQuery {
        &PostgresAccountRepository
    }
}

impl AccountModifier for PostgresAccountRepository {
    type Transaction = PostgresConnection;

    async fn create(
        &self,
        transaction: &mut Self::Transaction,
        account: &Account,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = transaction;
        sqlx::query(
            //language=postgresql
            r#"
            INSERT INTO accounts (id, name, private_key, public_key, is_bot, created_at)
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(account.id().as_ref())
        .bind(account.name().as_ref())
        .bind(account.private_key().as_ref())
        .bind(account.public_key().as_ref())
        .bind(account.is_bot().as_ref())
        .bind(account.created_at().as_ref())
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }

    async fn update(
        &self,
        transaction: &mut Self::Transaction,
        account: &Account,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = transaction;
        sqlx::query(
            //language=postgresql
            r#"
            UPDATE accounts
            SET name = $2, private_key = $3, public_key = $4, is_bot = $5, created_at = $6
            WHERE id = $1
            "#,
        )
        .bind(account.id().as_ref())
        .bind(account.name().as_ref())
        .bind(account.private_key().as_ref())
        .bind(account.public_key().as_ref())
        .bind(account.is_bot().as_ref())
        .bind(account.created_at().as_ref())
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }

    async fn delete(
        &self,
        transaction: &mut Self::Transaction,
        account_id: &AccountId,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = transaction;
        sqlx::query(
            //language=postgresql
            r#"
            UPDATE accounts
            SET deleted_at = CASE WHEN deleted_at IS NULL THEN now() ELSE deleted_at END
            WHERE id = $1
            "#,
        )
        .bind(account_id.as_ref())
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }
}

impl DependOnAccountModifier for PostgresDatabase {
    type AccountModifier = PostgresAccountRepository;

    fn account_modifier(&self) -> &Self::AccountModifier {
        &PostgresAccountRepository
    }
}

#[cfg(test)]
mod test {
    mod query {
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::modify::{AccountModifier, DependOnAccountModifier};
        use kernel::interfaces::query::{AccountQuery, DependOnAccountQuery};
        use kernel::prelude::entity::{
            Account, AccountId, AccountIsBot, AccountName, AccountPrivateKey, AccountPublicKey,
            CreatedAt, DeletedAt, StellarAccountId,
        };
        use sqlx::types::time::OffsetDateTime;
        use sqlx::types::Uuid;

        #[tokio::test]
        async fn find_by_id() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let id = AccountId::new(Uuid::new_v4());
            let account = Account::new(
                id.clone(),
                AccountName::new("test"),
                AccountPrivateKey::new("test"),
                AccountPublicKey::new("test"),
                AccountIsBot::new(false),
                CreatedAt::new(OffsetDateTime::now_utc()),
                None,
            );
            database
                .account_modifier()
                .create(&mut transaction, &account)
                .await
                .unwrap();
            let result = database
                .account_query()
                .find_by_id(&mut transaction, &id)
                .await
                .unwrap();
            assert_eq!(result, Some(account));
        }

        #[tokio::test]
        async fn find_by_stellar_id() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let accounts = database
                .account_query()
                .find_by_stellar_id(&mut transaction, &StellarAccountId::new(Uuid::new_v4()))
                .await
                .unwrap();
            assert!(accounts.is_empty());
        }

        #[tokio::test]
        async fn find_by_name() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let name = AccountName::new("test");
            let account = Account::new(
                AccountId::new(Uuid::new_v4()),
                name.clone(),
                AccountPrivateKey::new("test"),
                AccountPublicKey::new("test"),
                AccountIsBot::new(false),
                CreatedAt::new(OffsetDateTime::now_utc()),
                None,
            );
            database
                .account_modifier()
                .create(&mut transaction, &account)
                .await
                .unwrap();

            let result = database
                .account_query()
                .find_by_name(&mut transaction, &name)
                .await
                .unwrap();
            assert_eq!(result, Some(account));
        }
    }

    mod modify {
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::modify::{AccountModifier, DependOnAccountModifier};
        use kernel::interfaces::query::{AccountQuery, DependOnAccountQuery};
        use kernel::prelude::entity::{
            Account, AccountId, AccountIsBot, AccountName, AccountPrivateKey, AccountPublicKey,
            CreatedAt, DeletedAt, StellarAccountId,
        };
        use sqlx::types::time::OffsetDateTime;
        use sqlx::types::Uuid;

        #[tokio::test]
        async fn create() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let account = Account::new(
                AccountId::new(Uuid::new_v4()),
                AccountName::new("test"),
                AccountPrivateKey::new("test"),
                AccountPublicKey::new("test"),
                AccountIsBot::new(false),
                CreatedAt::new(OffsetDateTime::now_utc()),
                None,
            );
            database
                .account_modifier()
                .create(&mut transaction, &account)
                .await
                .unwrap();
            let result = database
                .account_query()
                .find_by_id(&mut transaction, account.id())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(result, account);
        }

        #[tokio::test]
        async fn update() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let account = Account::new(
                AccountId::new(Uuid::new_v4()),
                AccountName::new("test"),
                AccountPrivateKey::new("test"),
                AccountPublicKey::new("test"),
                AccountIsBot::new(false),
                CreatedAt::new(OffsetDateTime::now_utc()),
                None,
            );
            database
                .account_modifier()
                .create(&mut transaction, &account)
                .await
                .unwrap();
            let updated_account = Account::new(
                account.id().clone(),
                AccountName::new("test2"),
                AccountPrivateKey::new("test2"),
                AccountPublicKey::new("test2"),
                AccountIsBot::new(true),
                CreatedAt::new(OffsetDateTime::now_utc()),
                None,
            );
            database
                .account_modifier()
                .update(&mut transaction, &updated_account)
                .await
                .unwrap();
            let result = database
                .account_query()
                .find_by_id(&mut transaction, account.id())
                .await
                .unwrap();
            assert_eq!(result, Some(updated_account));
        }

        #[tokio::test]
        async fn delete() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let account = Account::new(
                AccountId::new(Uuid::new_v4()),
                AccountName::new("test"),
                AccountPrivateKey::new("test"),
                AccountPublicKey::new("test"),
                AccountIsBot::new(false),
                CreatedAt::new(OffsetDateTime::now_utc()),
                None,
            );
            database
                .account_modifier()
                .create(&mut transaction, &account)
                .await
                .unwrap();

            database
                .account_modifier()
                .delete(&mut transaction, account.id())
                .await
                .unwrap();
            let result = database
                .account_query()
                .find_by_id(&mut transaction, account.id())
                .await
                .unwrap()
                .unwrap();
            assert!(result.deleted_at().is_some());

            // Ignore if the account is already deleted
            let account = Account::new(
                AccountId::new(Uuid::new_v4()),
                AccountName::new("test"),
                AccountPrivateKey::new("test"),
                AccountPublicKey::new("test"),
                AccountIsBot::new(false),
                CreatedAt::new(OffsetDateTime::now_utc()),
                Some(DeletedAt::new(OffsetDateTime::now_utc())),
            );
            database
                .account_modifier()
                .create(&mut transaction, &account)
                .await
                .unwrap();

            database
                .account_modifier()
                .delete(&mut transaction, account.id())
                .await
                .unwrap();
            let result = database
                .account_query()
                .find_by_id(&mut transaction, account.id())
                .await
                .unwrap();
            assert_eq!(result, Some(account));
        }
    }
}
