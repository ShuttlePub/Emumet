use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use kernel::interfaces::repository::{AuthHostRepository, DependOnAuthHostRepository};
use kernel::prelude::entity::{AuthHost, AuthHostId, AuthHostUrl};
use kernel::KernelError;
use sqlx::PgConnection;
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct AuthHostRow {
    id: Uuid,
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
    use kernel::prelude::entity::AuthHostUrl;
    use uuid::Uuid;

    fn url() -> AuthHostUrl {
        AuthHostUrl::new(format!("https://{}.example.com", Uuid::now_v7()))
    }

    mod query {
        use crate::database::postgres::auth_host::test::url;
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::repository::{AuthHostRepository, DependOnAuthHostRepository};
        use kernel::prelude::entity::{AuthHost, AuthHostId};
        use uuid::Uuid;

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_id() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let auth_host = AuthHost::new(AuthHostId::new(Uuid::now_v7()), url());
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
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let auth_host = AuthHost::new(AuthHostId::new(Uuid::now_v7()), url());
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
        use crate::database::postgres::auth_host::test::url;
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::repository::{AuthHostRepository, DependOnAuthHostRepository};
        use kernel::prelude::entity::{AuthHost, AuthHostId};
        use uuid::Uuid;

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn create() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let auth_host = AuthHost::new(AuthHostId::new(Uuid::now_v7()), url());
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
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let auth_host = AuthHost::new(AuthHostId::new(Uuid::now_v7()), url());
            database
                .auth_host_repository()
                .create(&mut transaction, &auth_host)
                .await
                .unwrap();

            let updated_auth_host = AuthHost::new(auth_host.id().clone(), url());
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
