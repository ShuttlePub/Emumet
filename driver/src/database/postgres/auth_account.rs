use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use kernel::interfaces::read_model::{AuthAccountReadModel, DependOnAuthAccountReadModel};
use kernel::prelude::entity::{
    AuthAccount, AuthAccountClientId, AuthAccountId, AuthHostId, EventVersion,
};
use kernel::KernelError;
use sqlx::PgConnection;

#[derive(sqlx::FromRow)]
struct AuthAccountRow {
    id: i64,
    host_id: i64,
    client_id: String,
    version: i64,
}

impl From<AuthAccountRow> for AuthAccount {
    fn from(value: AuthAccountRow) -> Self {
        AuthAccount::new(
            AuthAccountId::new(value.id),
            AuthHostId::new(value.host_id),
            AuthAccountClientId::new(value.client_id),
            EventVersion::new(value.version),
        )
    }
}

pub struct PostgresAuthAccountReadModel;

impl AuthAccountReadModel for PostgresAuthAccountReadModel {
    type Executor = PostgresConnection;

    async fn find_by_id(
        &self,
        executor: &mut Self::Executor,
        account_id: &AuthAccountId,
    ) -> error_stack::Result<Option<AuthAccount>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, AuthAccountRow>(
            //language=postgresql
            r#"
            SELECT id, host_id, client_id, version
            FROM auth_accounts
            WHERE id = $1
            "#,
        )
        .bind(account_id.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()
        .map(|option| option.map(|row| row.into()))
    }

    async fn find_by_client_id(
        &self,
        executor: &mut Self::Executor,
        client_id: &AuthAccountClientId,
    ) -> error_stack::Result<Option<AuthAccount>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, AuthAccountRow>(
            //language=postgresql
            r#"
            SELECT id, host_id, client_id, version
            FROM auth_accounts
            WHERE client_id = $1
            "#,
        )
        .bind(client_id.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()
        .map(|option| option.map(|row| row.into()))
    }

    async fn create(
        &self,
        executor: &mut Self::Executor,
        auth_account: &AuthAccount,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            //language=postgresql
            r#"
            INSERT INTO auth_accounts (id, host_id, client_id, version) VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(auth_account.id().as_ref())
        .bind(auth_account.host().as_ref())
        .bind(auth_account.client_id().as_ref())
        .bind(auth_account.version().as_ref())
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }
}

impl DependOnAuthAccountReadModel for PostgresDatabase {
    type AuthAccountReadModel = PostgresAuthAccountReadModel;

    fn auth_account_read_model(&self) -> &Self::AuthAccountReadModel {
        &PostgresAuthAccountReadModel
    }
}

#[cfg(test)]
mod test {
    mod query {
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::read_model::{AuthAccountReadModel, DependOnAuthAccountReadModel};
        use kernel::interfaces::repository::{AuthHostRepository, DependOnAuthHostRepository};
        use kernel::prelude::entity::{AuthAccountId, AuthHostId};
        use kernel::test_utils::{AuthAccountBuilder, AuthHostBuilder};

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_id() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.get_executor().await.unwrap();

            let auth_host_id = AuthHostId::default();
            let auth_host = AuthHostBuilder::new().id(auth_host_id.clone()).build();
            database
                .auth_host_repository()
                .create(&mut transaction, &auth_host)
                .await
                .unwrap();
            let account_id = AuthAccountId::default();
            let auth_account = AuthAccountBuilder::new()
                .id(account_id.clone())
                .host(auth_host_id)
                .client_id("client_id")
                .build();

            database
                .auth_account_read_model()
                .create(&mut transaction, &auth_account)
                .await
                .unwrap();
            let result = database
                .auth_account_read_model()
                .find_by_id(&mut transaction, &account_id)
                .await
                .unwrap();
            assert_eq!(result, Some(auth_account));
        }
    }

    mod modify {
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::read_model::{AuthAccountReadModel, DependOnAuthAccountReadModel};
        use kernel::interfaces::repository::{AuthHostRepository, DependOnAuthHostRepository};
        use kernel::prelude::entity::{AuthAccountId, AuthHostId};
        use kernel::test_utils::{AuthAccountBuilder, AuthHostBuilder};

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn create() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.get_executor().await.unwrap();

            let host_id = AuthHostId::default();
            let account_id = AuthAccountId::default();
            let auth_host = AuthHostBuilder::new().id(host_id.clone()).build();
            database
                .auth_host_repository()
                .create(&mut transaction, &auth_host)
                .await
                .unwrap();
            let auth_account = AuthAccountBuilder::new()
                .id(account_id.clone())
                .host(host_id)
                .client_id("client_id")
                .build();
            database
                .auth_account_read_model()
                .create(&mut transaction, &auth_account)
                .await
                .unwrap();
            let result = database
                .auth_account_read_model()
                .find_by_id(&mut transaction, &account_id)
                .await
                .unwrap();
            assert_eq!(result, Some(auth_account));
        }
    }
}
