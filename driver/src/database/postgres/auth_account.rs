use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use kernel::interfaces::repository::{AuthAccountRepository, DependOnAuthAccountRepository};
use kernel::prelude::entity::{AuthAccount, AuthAccountClientId, AuthAccountId, AuthHostId};
use kernel::KernelError;
use sqlx::PgConnection;

#[derive(sqlx::FromRow)]
struct AuthAccountRow {
    id: i64,
    host_id: i64,
    client_id: String,
}

impl From<AuthAccountRow> for AuthAccount {
    fn from(value: AuthAccountRow) -> Self {
        AuthAccount::new(
            AuthAccountId::new(value.id),
            AuthHostId::new(value.host_id),
            AuthAccountClientId::new(value.client_id),
        )
    }
}

pub struct PostgresAuthAccountRepository;

impl AuthAccountRepository for PostgresAuthAccountRepository {
    type Connection = PostgresConnection;

    async fn find_or_create(
        &self,
        executor: &mut Self::Connection,
        host_id: &AuthHostId,
        client_id: &AuthAccountClientId,
    ) -> error_stack::Result<AuthAccount, KernelError> {
        let con: &mut PgConnection = executor;

        let inserted: Option<AuthAccountRow> = sqlx::query_as::<_, AuthAccountRow>(
            //language=postgresql
            r#"
            INSERT INTO auth_accounts (id, host_id, client_id)
            VALUES ($1, $2, $3)
            ON CONFLICT (host_id, client_id) DO NOTHING
            RETURNING id, host_id, client_id
            "#,
        )
        .bind(AuthAccountId::default().as_ref())
        .bind(host_id.as_ref())
        .bind(client_id.as_ref())
        .fetch_optional(&mut *con)
        .await
        .convert_error()?;

        if let Some(row) = inserted {
            return Ok(row.into());
        }

        let row = sqlx::query_as::<_, AuthAccountRow>(
            //language=postgresql
            r#"
            SELECT id, host_id, client_id
            FROM auth_accounts
            WHERE host_id = $1 AND client_id = $2
            "#,
        )
        .bind(host_id.as_ref())
        .bind(client_id.as_ref())
        .fetch_one(&mut *con)
        .await
        .convert_error()?;

        Ok(row.into())
    }

    async fn find_by_id(
        &self,
        executor: &mut Self::Connection,
        account_id: &AuthAccountId,
    ) -> error_stack::Result<Option<AuthAccount>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, AuthAccountRow>(
            //language=postgresql
            r#"
            SELECT id, host_id, client_id
            FROM auth_accounts
            WHERE id = $1
            "#,
        )
        .bind(account_id.as_ref())
        .fetch_optional(&mut *con)
        .await
        .convert_error()
        .map(|option| option.map(|row| row.into()))
    }

    async fn create(
        &self,
        executor: &mut Self::Connection,
        auth_account: &AuthAccount,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            //language=postgresql
            r#"
            INSERT INTO auth_accounts (id, host_id, client_id) VALUES ($1, $2, $3)
            "#,
        )
        .bind(auth_account.id().as_ref())
        .bind(auth_account.host().as_ref())
        .bind(auth_account.client_id().as_ref())
        .execute(&mut *con)
        .await
        .convert_error()?;
        Ok(())
    }
}

impl DependOnAuthAccountRepository for PostgresDatabase {
    type AuthAccountRepository = PostgresAuthAccountRepository;

    fn auth_account_repository(&self) -> &Self::AuthAccountRepository {
        &PostgresAuthAccountRepository
    }
}

#[cfg(test)]
mod test {
    use crate::database::PostgresDatabase;
    use kernel::interfaces::database::DatabaseConnection;
    use kernel::interfaces::repository::{AuthAccountRepository, DependOnAuthAccountRepository};
    use kernel::interfaces::repository::{AuthHostRepository, DependOnAuthHostRepository};
    use kernel::prelude::entity::{AuthAccountClientId, AuthAccountId, AuthHostId};
    use kernel::test_utils::{AuthAccountBuilder, AuthHostBuilder};
    use std::sync::Arc;

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn find_by_id() {
        kernel::ensure_generator_initialized();
        let database = PostgresDatabase::new().await.unwrap();
        let mut conn = database.connection().await.unwrap();

        let auth_host_id = AuthHostId::default();
        let auth_host = AuthHostBuilder::new().id(auth_host_id.clone()).build();
        database
            .auth_host_repository()
            .create(&mut conn, &auth_host)
            .await
            .unwrap();
        let account_id = AuthAccountId::default();
        let auth_account = AuthAccountBuilder::new()
            .id(account_id.clone())
            .host(auth_host_id)
            .client_id("client_id")
            .build();

        database
            .auth_account_repository()
            .create(&mut conn, &auth_account)
            .await
            .unwrap();
        let result = database
            .auth_account_repository()
            .find_by_id(&mut conn, &account_id)
            .await
            .unwrap();
        assert_eq!(result, Some(auth_account));
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn find_or_create_creates_new_account() {
        kernel::ensure_generator_initialized();
        let database = PostgresDatabase::new().await.unwrap();
        let mut conn = database.connection().await.unwrap();

        let host_id = AuthHostId::default();
        let auth_host = AuthHostBuilder::new().id(host_id.clone()).build();
        database
            .auth_host_repository()
            .create(&mut conn, &auth_host)
            .await
            .unwrap();

        let client_id = AuthAccountClientId::new("client_id");
        let account = database
            .auth_account_repository()
            .find_or_create(&mut conn, &host_id, &client_id)
            .await
            .unwrap();

        assert_eq!(account.host(), &host_id);
        assert_eq!(account.client_id(), &client_id);

        let found = database
            .auth_account_repository()
            .find_by_id(&mut conn, account.id())
            .await
            .unwrap();
        assert_eq!(found, Some(account));
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn find_or_create_is_idempotent() {
        kernel::ensure_generator_initialized();
        let database = PostgresDatabase::new().await.unwrap();
        let mut conn = database.connection().await.unwrap();

        let host_id = AuthHostId::default();
        let auth_host = AuthHostBuilder::new().id(host_id.clone()).build();
        database
            .auth_host_repository()
            .create(&mut conn, &auth_host)
            .await
            .unwrap();

        let client_id = AuthAccountClientId::new("client_id");
        let first = database
            .auth_account_repository()
            .find_or_create(&mut conn, &host_id, &client_id)
            .await
            .unwrap();
        let second = database
            .auth_account_repository()
            .find_or_create(&mut conn, &host_id, &client_id)
            .await
            .unwrap();

        assert_eq!(first.id(), second.id());
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn find_or_create_concurrent_race_returns_same_id_and_one_row() {
        kernel::ensure_generator_initialized();
        let database = PostgresDatabase::new().await.unwrap();

        let host_id = AuthHostId::default();
        let auth_host = AuthHostBuilder::new().id(host_id.clone()).build();
        {
            let mut conn = database.connection().await.unwrap();
            database
                .auth_host_repository()
                .create(&mut conn, &auth_host)
                .await
                .unwrap();
        }

        let client_id = AuthAccountClientId::new("concurrent-client-id");
        let db = Arc::new(database);
        let mut handles = Vec::new();
        for _ in 0..10 {
            let db = db.clone();
            let host_id = host_id.clone();
            let client_id = client_id.clone();
            handles.push(tokio::spawn(async move {
                let mut conn = db.connection().await.unwrap();
                db.auth_account_repository()
                    .find_or_create(&mut conn, &host_id, &client_id)
                    .await
                    .unwrap()
            }));
        }

        let results = futures::future::join_all(handles).await;
        let ids: Vec<_> = results
            .into_iter()
            .map(|r| r.unwrap().id().clone())
            .collect();
        assert!(
            ids.iter().all(|id| id == &ids[0]),
            "all concurrent calls must return the same id"
        );

        let mut conn = db.connection().await.unwrap();
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM auth_accounts WHERE host_id = $1 AND client_id = $2",
        )
        .bind(host_id.as_ref())
        .bind(client_id.as_ref())
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(count, 1, "exactly one row must exist");
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn create() {
        kernel::ensure_generator_initialized();
        let database = PostgresDatabase::new().await.unwrap();
        let mut conn = database.connection().await.unwrap();

        let host_id = AuthHostId::default();
        let account_id = AuthAccountId::default();
        let auth_host = AuthHostBuilder::new().id(host_id.clone()).build();
        database
            .auth_host_repository()
            .create(&mut conn, &auth_host)
            .await
            .unwrap();
        let auth_account = AuthAccountBuilder::new()
            .id(account_id.clone())
            .host(host_id)
            .client_id("client_id")
            .build();
        database
            .auth_account_repository()
            .create(&mut conn, &auth_account)
            .await
            .unwrap();
        let result = database
            .auth_account_repository()
            .find_by_id(&mut conn, &account_id)
            .await
            .unwrap();
        assert_eq!(result, Some(auth_account));
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn migration_dropped_events_table_and_version_column() {
        kernel::ensure_generator_initialized();
        let database = PostgresDatabase::new().await.unwrap();
        let mut conn = database.connection().await.unwrap();

        let events_table_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM information_schema.tables
                WHERE table_schema = 'public' AND table_name = 'auth_account_events'
            )",
        )
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert!(!events_table_exists, "auth_account_events must be dropped");

        let version_column_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM information_schema.columns
                WHERE table_schema = 'public'
                  AND table_name = 'auth_accounts'
                  AND column_name = 'version'
            )",
        )
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert!(
            !version_column_exists,
            "auth_accounts.version must be dropped"
        );
    }
}
