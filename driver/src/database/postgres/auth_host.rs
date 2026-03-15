use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use kernel::interfaces::repository::{AuthHostRepository, DependOnAuthHostRepository};
use kernel::prelude::entity::{AuthHost, AuthHostId, AuthHostUrl};
use kernel::KernelError;
use sqlx::PgConnection;

#[derive(sqlx::FromRow)]
struct AuthHostRow {
    id: i64,
    url: String,
}

impl From<AuthHostRow> for AuthHost {
    fn from(row: AuthHostRow) -> Self {
        AuthHost::new(AuthHostId::new(row.id), AuthHostUrl::new(row.url))
    }
}

pub struct PostgresAuthHostRepository;

impl AuthHostRepository for PostgresAuthHostRepository {
    type Executor = PostgresConnection;
    async fn find_by_id(
        &self,
        executor: &mut Self::Executor,
        id: &AuthHostId,
    ) -> error_stack::Result<Option<AuthHost>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, AuthHostRow>(
            // language=postgresql
            r#"
            SELECT id, url
            FROM auth_hosts
            WHERE id = $1
            "#,
        )
        .bind(id.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()
        .map(|row| row.map(AuthHost::from))
    }

    async fn find_by_url(
        &self,
        executor: &mut Self::Executor,
        domain: &AuthHostUrl,
    ) -> error_stack::Result<Option<AuthHost>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, AuthHostRow>(
            // language=postgresql
            r#"
            SELECT id, url
            FROM auth_hosts
            WHERE url = $1
            "#,
        )
        .bind(domain.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()
        .map(|row| row.map(AuthHost::from))
    }

    async fn create(
        &self,
        executor: &mut Self::Executor,
        auth_host: &AuthHost,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            // language=postgresql
            r#"
            INSERT INTO auth_hosts (id, url)
            VALUES ($1, $2)
            "#,
        )
        .bind(auth_host.id().as_ref())
        .bind(auth_host.url().as_ref())
        .execute(con)
        .await
        .convert_error()
        .map(|_| ())
    }

    async fn update(
        &self,
        executor: &mut Self::Executor,
        auth_host: &AuthHost,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            // language=postgresql
            r#"
            UPDATE auth_hosts
            SET url = $2
            WHERE id = $1
            "#,
        )
        .bind(auth_host.id().as_ref())
        .bind(auth_host.url().as_ref())
        .execute(con)
        .await
        .convert_error()
        .map(|_| ())
    }
}

impl DependOnAuthHostRepository for PostgresDatabase {
    type AuthHostRepository = PostgresAuthHostRepository;

    fn auth_host_repository(&self) -> &Self::AuthHostRepository {
        &PostgresAuthHostRepository
    }
}

#[cfg(test)]
mod test {
    mod query {
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::repository::{AuthHostRepository, DependOnAuthHostRepository};
        use kernel::test_utils::AuthHostBuilder;

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_id() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.get_executor().await.unwrap();

            let auth_host = AuthHostBuilder::new().build();
            database
                .auth_host_repository()
                .create(&mut transaction, &auth_host)
                .await
                .unwrap();

            let found_auth_host = database
                .auth_host_repository()
                .find_by_id(&mut transaction, auth_host.id())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(auth_host, found_auth_host);
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_url() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.get_executor().await.unwrap();

            let auth_host = AuthHostBuilder::new().build();
            database
                .auth_host_repository()
                .create(&mut transaction, &auth_host)
                .await
                .unwrap();

            let found_auth_host = database
                .auth_host_repository()
                .find_by_url(&mut transaction, auth_host.url())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(auth_host, found_auth_host);
        }
    }

    mod modify {
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::repository::{AuthHostRepository, DependOnAuthHostRepository};
        use kernel::test_utils::{unique_auth_host_url, AuthHostBuilder};

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn create() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.get_executor().await.unwrap();

            let auth_host = AuthHostBuilder::new().build();
            database
                .auth_host_repository()
                .create(&mut transaction, &auth_host)
                .await
                .unwrap();

            let found_auth_host = database
                .auth_host_repository()
                .find_by_id(&mut transaction, auth_host.id())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(auth_host, found_auth_host);
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn update() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.get_executor().await.unwrap();

            let auth_host = AuthHostBuilder::new().build();
            database
                .auth_host_repository()
                .create(&mut transaction, &auth_host)
                .await
                .unwrap();

            let new_url = unique_auth_host_url();
            let updated_auth_host = AuthHostBuilder::new()
                .id(auth_host.id().clone())
                .url(new_url.as_ref())
                .build();
            database
                .auth_host_repository()
                .update(&mut transaction, &updated_auth_host)
                .await
                .unwrap();

            let found_auth_host = database
                .auth_host_repository()
                .find_by_id(&mut transaction, auth_host.id())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(updated_auth_host, found_auth_host);
        }
    }
}
