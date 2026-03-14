use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use kernel::interfaces::read_model::{AccountReadModel, DependOnAccountReadModel};
use kernel::prelude::entity::{
    Account, AccountId, AccountIsBot, AccountName, AccountPrivateKey, AccountPublicKey,
    AccountStatus, AuthAccountId, CreatedAt, DeletedAt, EventVersion, Nanoid,
};
use kernel::KernelError;
use sqlx::types::time::OffsetDateTime;
use sqlx::PgConnection;

#[derive(sqlx::FromRow)]
struct AccountRow {
    id: i64,
    name: String,
    private_key: String,
    public_key: String,
    is_bot: bool,
    deleted_at: Option<OffsetDateTime>,
    version: i64,
    nanoid: String,
    created_at: OffsetDateTime,
    suspended_at: Option<OffsetDateTime>,
    suspend_expires_at: Option<OffsetDateTime>,
    suspend_reason: Option<String>,
    banned_at: Option<OffsetDateTime>,
    ban_reason: Option<String>,
}

impl From<AccountRow> for Account {
    fn from(value: AccountRow) -> Self {
        let status = if let (Some(banned_at), Some(reason)) = (value.banned_at, value.ban_reason) {
            AccountStatus::Banned { reason, banned_at }
        } else if let (Some(suspended_at), Some(reason)) =
            (value.suspended_at, value.suspend_reason.clone())
        {
            // If suspend has expired, treat as Active
            if let Some(expires_at) = value.suspend_expires_at {
                if expires_at <= OffsetDateTime::now_utc() {
                    AccountStatus::Active
                } else {
                    AccountStatus::Suspended {
                        reason,
                        suspended_at,
                        expires_at: Some(expires_at),
                    }
                }
            } else {
                AccountStatus::Suspended {
                    reason,
                    suspended_at,
                    expires_at: None,
                }
            }
        } else {
            AccountStatus::Active
        };

        Account::new(
            AccountId::new(value.id),
            AccountName::new(value.name),
            AccountPrivateKey::new(value.private_key),
            AccountPublicKey::new(value.public_key),
            AccountIsBot::new(value.is_bot),
            status,
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
            SELECT id, name, private_key, public_key, is_bot, deleted_at, version, nanoid, created_at,
                   suspended_at, suspend_expires_at, suspend_reason, banned_at, ban_reason
            FROM accounts
            WHERE id = $1 AND deleted_at IS NULL
              AND banned_at IS NULL
              AND (suspended_at IS NULL OR (suspend_expires_at IS NOT NULL AND suspend_expires_at <= now()))
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
            -- Intentionally does NOT filter suspended/banned: allows account owners
            -- to see their own accounts' moderation status via the listing endpoint.
            SELECT accounts.id, name, private_key, public_key, is_bot, deleted_at, version, nanoid, created_at,
                   suspended_at, suspend_expires_at, suspend_reason, banned_at, ban_reason
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
            SELECT id, name, private_key, public_key, is_bot, deleted_at, version, nanoid, created_at,
                   suspended_at, suspend_expires_at, suspend_reason, banned_at, ban_reason
            FROM accounts
            WHERE name = $1 AND deleted_at IS NULL
              AND banned_at IS NULL
              AND (suspended_at IS NULL OR (suspend_expires_at IS NOT NULL AND suspend_expires_at <= now()))
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
            SELECT id, name, private_key, public_key, is_bot, deleted_at, version, nanoid, created_at,
                   suspended_at, suspend_expires_at, suspend_reason, banned_at, ban_reason
            FROM accounts
            WHERE nanoid = $1 AND deleted_at IS NULL
              AND banned_at IS NULL
              AND (suspended_at IS NULL OR (suspend_expires_at IS NOT NULL AND suspend_expires_at <= now()))
            "#,
        )
        .bind(nanoid.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()
        .map(|option| option.map(Account::from))
    }

    async fn find_by_nanoids(
        &self,
        executor: &mut Self::Executor,
        nanoids: &[Nanoid<Account>],
    ) -> error_stack::Result<Vec<Account>, KernelError> {
        let con: &mut PgConnection = executor;
        let nanoid_strs: Vec<&str> = nanoids.iter().map(|n| n.as_ref().as_str()).collect();
        sqlx::query_as::<_, AccountRow>(
            //language=postgresql
            r#"
            SELECT id, name, private_key, public_key, is_bot, deleted_at, version, nanoid, created_at,
                   suspended_at, suspend_expires_at, suspend_reason, banned_at, ban_reason
            FROM accounts
            WHERE nanoid = ANY($1) AND deleted_at IS NULL
              AND banned_at IS NULL
              AND (suspended_at IS NULL OR (suspend_expires_at IS NOT NULL AND suspend_expires_at <= now()))
            "#,
        )
        .bind(&nanoid_strs)
        .fetch_all(con)
        .await
        .convert_error()
        .map(|rows| rows.into_iter().map(Account::from).collect())
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
        let (suspended_at, suspend_expires_at, suspend_reason, banned_at, ban_reason) =
            match account.status() {
                AccountStatus::Active => (None, None, None, None, None),
                AccountStatus::Suspended {
                    reason,
                    suspended_at,
                    expires_at,
                } => (
                    Some(*suspended_at),
                    *expires_at,
                    Some(reason.clone()),
                    None,
                    None,
                ),
                AccountStatus::Banned { reason, banned_at } => {
                    (None, None, None, Some(*banned_at), Some(reason.clone()))
                }
            };
        sqlx::query(
            //language=postgresql
            r#"
            UPDATE accounts
            SET name = $2, private_key = $3, public_key = $4, is_bot = $5, version = $6, deleted_at = $7,
                suspended_at = $8, suspend_expires_at = $9, suspend_reason = $10,
                banned_at = $11, ban_reason = $12
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
        .bind(suspended_at)
        .bind(suspend_expires_at)
        .bind(suspend_reason)
        .bind(banned_at)
        .bind(ban_reason)
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

    async fn find_by_id_unfiltered(
        &self,
        executor: &mut Self::Executor,
        id: &AccountId,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, AccountRow>(
            //language=postgresql
            r#"
            SELECT id, name, private_key, public_key, is_bot, deleted_at, version, nanoid, created_at,
                   suspended_at, suspend_expires_at, suspend_reason, banned_at, ban_reason
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

    async fn find_by_nanoid_unfiltered(
        &self,
        executor: &mut Self::Executor,
        nanoid: &Nanoid<Account>,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, AccountRow>(
            //language=postgresql
            r#"
            SELECT id, name, private_key, public_key, is_bot, deleted_at, version, nanoid, created_at,
                   suspended_at, suspend_expires_at, suspend_reason, banned_at, ban_reason
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

    async fn find_by_nanoids_unfiltered(
        &self,
        executor: &mut Self::Executor,
        nanoids: &[Nanoid<Account>],
    ) -> error_stack::Result<Vec<Account>, KernelError> {
        let con: &mut PgConnection = executor;
        let nanoid_strs: Vec<&str> = nanoids.iter().map(|n| n.as_ref().as_str()).collect();
        sqlx::query_as::<_, AccountRow>(
            //language=postgresql
            r#"
            SELECT id, name, private_key, public_key, is_bot, deleted_at, version, nanoid, created_at,
                   suspended_at, suspend_expires_at, suspend_reason, banned_at, ban_reason
            FROM accounts
            WHERE nanoid = ANY($1) AND deleted_at IS NULL
            "#,
        )
        .bind(&nanoid_strs)
        .fetch_all(con)
        .await
        .convert_error()
        .map(|rows| rows.into_iter().map(Account::from).collect())
    }

    async fn suspend(
        &self,
        executor: &mut Self::Executor,
        account_id: &AccountId,
        reason: &str,
        expires_at: Option<OffsetDateTime>,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            //language=postgresql
            r#"
            UPDATE accounts
            SET suspended_at = now(), suspend_expires_at = $2, suspend_reason = $3
            WHERE id = $1
            "#,
        )
        .bind(account_id.as_ref())
        .bind(expires_at)
        .bind(reason)
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }

    async fn unsuspend(
        &self,
        executor: &mut Self::Executor,
        account_id: &AccountId,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            //language=postgresql
            r#"
            UPDATE accounts
            SET suspended_at = NULL, suspend_expires_at = NULL, suspend_reason = NULL
            WHERE id = $1
            "#,
        )
        .bind(account_id.as_ref())
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }

    async fn ban(
        &self,
        executor: &mut Self::Executor,
        account_id: &AccountId,
        reason: &str,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            //language=postgresql
            r#"
            UPDATE accounts
            SET banned_at = now(), ban_reason = $2,
                suspended_at = NULL, suspend_expires_at = NULL, suspend_reason = NULL
            WHERE id = $1
            "#,
        )
        .bind(account_id.as_ref())
        .bind(reason)
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

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_id() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let id = AccountId::default();
            let account = Account::new(
                id.clone(),
                AccountName::new("test"),
                AccountPrivateKey::new("test"),
                AccountPublicKey::new("test"),
                AccountIsBot::new(false),
                Default::default(),
                None,
                EventVersion::default(),
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
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let accounts = database
                .account_read_model()
                .find_by_auth_id(&mut transaction, &AuthAccountId::default())
                .await
                .unwrap();
            assert!(accounts.is_empty());
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_name() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let name = AccountName::new(nanoid::nanoid!());
            let account = Account::new(
                AccountId::default(),
                name.clone(),
                AccountPrivateKey::new("test"),
                AccountPublicKey::new("test"),
                AccountIsBot::new(false),
                Default::default(),
                None,
                EventVersion::default(),
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
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let nanoid = Nanoid::default();
            let account = Account::new(
                AccountId::default(),
                AccountName::new("test"),
                AccountPrivateKey::new("test"),
                AccountPublicKey::new("test"),
                AccountIsBot::new(false),
                Default::default(),
                None,
                EventVersion::default(),
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
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let account = Account::new(
                AccountId::default(),
                AccountName::new("test"),
                AccountPrivateKey::new("test"),
                AccountPublicKey::new("test"),
                AccountIsBot::new(false),
                Default::default(),
                None,
                EventVersion::default(),
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
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let account = Account::new(
                AccountId::default(),
                AccountName::new("test"),
                AccountPrivateKey::new("test"),
                AccountPublicKey::new("test"),
                AccountIsBot::new(false),
                Default::default(),
                None,
                EventVersion::default(),
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
                Default::default(),
                None,
                EventVersion::default(),
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
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let account = Account::new(
                AccountId::default(),
                AccountName::new("test"),
                AccountPrivateKey::new("test"),
                AccountPublicKey::new("test"),
                AccountIsBot::new(false),
                Default::default(),
                None,
                EventVersion::default(),
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
                AccountId::default(),
                AccountName::new("test"),
                AccountPrivateKey::new("test"),
                AccountPublicKey::new("test"),
                AccountIsBot::new(false),
                Default::default(),
                Some(DeletedAt::new(OffsetDateTime::now_utc())),
                EventVersion::default(),
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
