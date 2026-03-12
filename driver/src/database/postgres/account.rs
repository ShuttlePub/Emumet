use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use kernel::interfaces::read_model::{AccountReadModel, DependOnAccountReadModel};
use kernel::prelude::entity::{
    Account, AccountId, AccountIsBot, AccountName, AccountPrivateKey, AccountPublicKey,
    AuthAccountId, CreatedAt, DeletedAt, EventVersion, Nanoid,
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
    deleted_at: Option<OffsetDateTime>,
    version: Uuid,
    nanoid: String,
    created_at: OffsetDateTime,
}

impl From<AccountRow> for Account {
    fn from(value: AccountRow) -> Self {
        Account::new(
            AccountId::new(value.id),
            AccountName::new(value.name),
            AccountPrivateKey::new(value.private_key),
            AccountPublicKey::new(value.public_key),
            AccountIsBot::new(value.is_bot),
            value.deleted_at.map(DeletedAt::new),
            EventVersion::new(value.version),
            Nanoid::new(value.nanoid),
            CreatedAt::new(value.created_at),
        )
    }
}

pub struct PostgresAccountReadModel;

impl AccountReadModel for PostgresAccountReadModel {
    type Executor = PostgresConnection;

    async fn find_by_id(
        &self,
        executor: &mut Self::Executor,
        id: &AccountId,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, AccountRow>(
            //language=postgresql
            r#"
            SELECT id, name, private_key, public_key, is_bot, deleted_at, version, nanoid, created_at
            FROM accounts
            WHERE id = $1 AND deleted_at IS NULL
            "#,
        )
        .bind(id.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()
        .map(|option| option.map(Account::from))
    }

    async fn find_by_auth_id(
        &self,
        executor: &mut Self::Executor,
        auth_id: &AuthAccountId,
    ) -> error_stack::Result<Vec<Account>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, AccountRow>(
            //language=postgresql
            r#"
            SELECT id, name, private_key, public_key, is_bot, deleted_at, version, nanoid, created_at
            FROM accounts
            INNER JOIN auth_emumet_accounts ON auth_emumet_accounts.emumet_id = accounts.id
            WHERE auth_emumet_accounts.auth_id = $1 AND deleted_at IS NULL
            "#,
        )
        .bind(auth_id.as_ref())
        .fetch_all(con)
        .await
        .convert_error()
        .map(|rows| rows.into_iter().map(Account::from).collect())
    }

    async fn find_by_name(
        &self,
        executor: &mut Self::Executor,
        name: &AccountName,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, AccountRow>(
            //language=postgresql
            r#"
            SELECT id, name, private_key, public_key, is_bot, deleted_at, version, nanoid, created_at
            FROM accounts
            WHERE name = $1 AND deleted_at IS NULL
            "#,
        )
        .bind(name.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()
        .map(|option| option.map(Account::from))
    }

    async fn find_by_nanoid(
        &self,
        executor: &mut Self::Executor,
        nanoid: &Nanoid<Account>,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, AccountRow>(
            //language=postgresql
            r#"
            SELECT id, name, private_key, public_key, is_bot, deleted_at, version, nanoid, created_at
            FROM accounts
            WHERE nanoid = $1 AND deleted_at IS NULL
            "#,
        )
        .bind(nanoid.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()
        .map(|option| option.map(Account::from))
    }

    async fn create(
        &self,
        executor: &mut Self::Executor,
        account: &Account,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            //language=postgresql
            r#"
            INSERT INTO accounts (id, name, private_key, public_key, is_bot, version, nanoid, created_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
        )
        .bind(account.id().as_ref())
        .bind(account.name().as_ref())
        .bind(account.private_key().as_ref())
        .bind(account.public_key().as_ref())
        .bind(account.is_bot().as_ref())
        .bind(account.version().as_ref())
        .bind(account.nanoid().as_ref())
        .bind(account.created_at().as_ref())
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }

    async fn update(
        &self,
        executor: &mut Self::Executor,
        account: &Account,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            //language=postgresql
            r#"
            UPDATE accounts
            SET name = $2, private_key = $3, public_key = $4, is_bot = $5, version = $6, deleted_at = $7
            WHERE id = $1
            "#,
        )
        .bind(account.id().as_ref())
        .bind(account.name().as_ref())
        .bind(account.private_key().as_ref())
        .bind(account.public_key().as_ref())
        .bind(account.is_bot().as_ref())
        .bind(account.version().as_ref())
        .bind(account.deleted_at().as_ref().map(|d| d.as_ref()))
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }

    async fn deactivate(
        &self,
        executor: &mut Self::Executor,
        account_id: &AccountId,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
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

    async fn unlink_all_auth_accounts(
        &self,
        executor: &mut Self::Executor,
        account_id: &AccountId,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            //language=postgresql
            r#"
            DELETE FROM auth_emumet_accounts WHERE emumet_id = $1
            "#,
        )
        .bind(account_id.as_ref())
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }

    async fn link_auth_account(
        &self,
        executor: &mut Self::Executor,
        account_id: &AccountId,
        auth_account_id: &AuthAccountId,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            //language=postgresql
            r#"
            INSERT INTO auth_emumet_accounts (emumet_id, auth_id) VALUES ($1, $2)
            "#,
        )
        .bind(account_id.as_ref())
        .bind(auth_account_id.as_ref())
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }
}

impl DependOnAccountReadModel for PostgresDatabase {
    type AccountReadModel = PostgresAccountReadModel;

    fn account_read_model(&self) -> &Self::AccountReadModel {
        &PostgresAccountReadModel
    }
}

#[cfg(test)]
mod test {
    mod read_model {
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::read_model::{AccountReadModel, DependOnAccountReadModel};
        use kernel::prelude::entity::{
            Account, AccountId, AccountIsBot, AccountName, AccountPrivateKey, AccountPublicKey,
            AuthAccountId, CreatedAt, DeletedAt, EventVersion, Nanoid,
        };
        use sqlx::types::time::OffsetDateTime;
        use sqlx::types::Uuid;

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_id() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let id = AccountId::new(Uuid::now_v7());
            let account = Account::new(
                id.clone(),
                AccountName::new("test"),
                AccountPrivateKey::new("test"),
                AccountPublicKey::new("test"),
                AccountIsBot::new(false),
                None,
                EventVersion::new(Uuid::now_v7()),
                Nanoid::default(),
                CreatedAt::now(),
            );
            database
                .account_read_model()
                .create(&mut transaction, &account)
                .await
                .unwrap();
            let result = database
                .account_read_model()
                .find_by_id(&mut transaction, &id)
                .await
                .unwrap();
            assert_eq!(result.as_ref().map(Account::id), Some(account.id()));
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_auth_id() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let accounts = database
                .account_read_model()
                .find_by_auth_id(&mut transaction, &AuthAccountId::new(Uuid::now_v7()))
                .await
                .unwrap();
            assert!(accounts.is_empty());
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_name() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let name = AccountName::new(Uuid::now_v7().to_string());
            let account = Account::new(
                AccountId::new(Uuid::now_v7()),
                name.clone(),
                AccountPrivateKey::new("test"),
                AccountPublicKey::new("test"),
                AccountIsBot::new(false),
                None,
                EventVersion::new(Uuid::now_v7()),
                Nanoid::default(),
                CreatedAt::now(),
            );
            database
                .account_read_model()
                .create(&mut transaction, &account)
                .await
                .unwrap();

            let result = database
                .account_read_model()
                .find_by_name(&mut transaction, &name)
                .await
                .unwrap();
            assert_eq!(result.as_ref().map(Account::id), Some(account.id()));
            database
                .account_read_model()
                .deactivate(&mut transaction, account.id())
                .await
                .unwrap();
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_nanoid() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let nanoid = Nanoid::default();
            let account = Account::new(
                AccountId::new(Uuid::now_v7()),
                AccountName::new("test"),
                AccountPrivateKey::new("test"),
                AccountPublicKey::new("test"),
                AccountIsBot::new(false),
                None,
                EventVersion::new(Uuid::now_v7()),
                nanoid.clone(),
                CreatedAt::now(),
            );
            database
                .account_read_model()
                .create(&mut transaction, &account)
                .await
                .unwrap();

            let result = database
                .account_read_model()
                .find_by_nanoid(&mut transaction, &nanoid)
                .await
                .unwrap();
            assert_eq!(result.as_ref().map(Account::id), Some(account.id()));
            database
                .account_read_model()
                .deactivate(&mut transaction, account.id())
                .await
                .unwrap();
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn create() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let account = Account::new(
                AccountId::new(Uuid::now_v7()),
                AccountName::new("test"),
                AccountPrivateKey::new("test"),
                AccountPublicKey::new("test"),
                AccountIsBot::new(false),
                None,
                EventVersion::new(Uuid::now_v7()),
                Nanoid::default(),
                CreatedAt::now(),
            );
            database
                .account_read_model()
                .create(&mut transaction, &account)
                .await
                .unwrap();
            let result = database
                .account_read_model()
                .find_by_id(&mut transaction, account.id())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(result.id(), account.id());
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn update() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let account = Account::new(
                AccountId::new(Uuid::now_v7()),
                AccountName::new("test"),
                AccountPrivateKey::new("test"),
                AccountPublicKey::new("test"),
                AccountIsBot::new(false),
                None,
                EventVersion::new(Uuid::now_v7()),
                Nanoid::default(),
                CreatedAt::now(),
            );
            database
                .account_read_model()
                .create(&mut transaction, &account)
                .await
                .unwrap();
            let updated_account = Account::new(
                account.id().clone(),
                AccountName::new("test2"),
                AccountPrivateKey::new("test2"),
                AccountPublicKey::new("test2"),
                AccountIsBot::new(true),
                None,
                EventVersion::new(Uuid::now_v7()),
                Nanoid::default(),
                CreatedAt::now(),
            );
            database
                .account_read_model()
                .update(&mut transaction, &updated_account)
                .await
                .unwrap();
            let result = database
                .account_read_model()
                .find_by_id(&mut transaction, account.id())
                .await
                .unwrap();
            assert_eq!(result.as_ref().map(Account::id), Some(updated_account.id()));
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn deactivate() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let account = Account::new(
                AccountId::new(Uuid::now_v7()),
                AccountName::new("test"),
                AccountPrivateKey::new("test"),
                AccountPublicKey::new("test"),
                AccountIsBot::new(false),
                None,
                EventVersion::new(Uuid::now_v7()),
                Nanoid::default(),
                CreatedAt::now(),
            );
            database
                .account_read_model()
                .create(&mut transaction, &account)
                .await
                .unwrap();

            database
                .account_read_model()
                .deactivate(&mut transaction, account.id())
                .await
                .unwrap();
            let result = database
                .account_read_model()
                .find_by_id(&mut transaction, account.id())
                .await
                .unwrap();
            assert!(result.is_none());

            // Ignore if the account is already deleted
            let account = Account::new(
                AccountId::new(Uuid::now_v7()),
                AccountName::new("test"),
                AccountPrivateKey::new("test"),
                AccountPublicKey::new("test"),
                AccountIsBot::new(false),
                Some(DeletedAt::new(OffsetDateTime::now_utc())),
                EventVersion::new(Uuid::now_v7()),
                Nanoid::default(),
                CreatedAt::now(),
            );
            database
                .account_read_model()
                .create(&mut transaction, &account)
                .await
                .unwrap();

            database
                .account_read_model()
                .deactivate(&mut transaction, account.id())
                .await
                .unwrap();
            let result = database
                .account_read_model()
                .find_by_id(&mut transaction, account.id())
                .await
                .unwrap();
            assert!(result.is_none());
        }
    }
}
