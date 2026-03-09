use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use kernel::interfaces::read_model::{AuthAccountReadModel, DependOnAuthAccountReadModel};
use kernel::prelude::entity::{
    AuthAccount, AuthAccountClientId, AuthAccountId, AuthHostId, EventVersion,
};
use kernel::KernelError;
use sqlx::PgConnection;
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct AuthAccountRow {
    id: Uuid,
    host_id: Uuid,
    client_id: String,
    version: Uuid,
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

    async fn update(
        &self,
        executor: &mut Self::Executor,
        auth_account: &AuthAccount,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            //language=postgresql
            r#"
            UPDATE auth_accounts SET host_id = $2, client_id = $3, version = $4
            WHERE id = $1
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

    async fn delete(
        &self,
        executor: &mut Self::Executor,
        account_id: &AuthAccountId,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            //language=postgresql
            r#"
            DELETE FROM auth_accounts WHERE id = $1
            "#,
        )
        .bind(account_id.as_ref())
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
        use kernel::interfaces::modify::{AuthHostModifier, DependOnAuthHostModifier};
        use kernel::interfaces::read_model::{AuthAccountReadModel, DependOnAuthAccountReadModel};
        use kernel::prelude::entity::{
            AuthAccount, AuthAccountClientId, AuthAccountId, AuthHost, AuthHostId, AuthHostUrl,
            EventVersion,
        };
        use uuid::Uuid;

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_id() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let auth_host_id = AuthHostId::new(Uuid::now_v7());
            let auth_host = AuthHost::new(auth_host_id.clone(), AuthHostUrl::new(Uuid::now_v7()));
            database
                .auth_host_modifier()
                .create(&mut transaction, &auth_host)
                .await
                .unwrap();
            let account_id = AuthAccountId::new(Uuid::now_v7());
            let auth_account = AuthAccount::new(
                account_id.clone(),
                auth_host_id,
                AuthAccountClientId::new("client_id".to_string()),
                EventVersion::new(Uuid::now_v7()),
            );

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
            assert_eq!(result, Some(auth_account.clone()));
            database
                .auth_account_read_model()
                .delete(&mut transaction, auth_account.id())
                .await
                .unwrap();
        }
    }

    mod modify {
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::modify::{AuthHostModifier, DependOnAuthHostModifier};
        use kernel::interfaces::read_model::{AuthAccountReadModel, DependOnAuthAccountReadModel};
        use kernel::prelude::entity::{
            AuthAccount, AuthAccountClientId, AuthAccountId, AuthHost, AuthHostId, AuthHostUrl,
            EventVersion,
        };
        use uuid::Uuid;

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn create() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let host_id = AuthHostId::new(Uuid::now_v7());
            let account_id = AuthAccountId::new(Uuid::now_v7());
            let auth_host = AuthHost::new(host_id.clone(), AuthHostUrl::new(Uuid::now_v7()));
            database
                .auth_host_modifier()
                .create(&mut transaction, &auth_host)
                .await
                .unwrap();
            let auth_account = AuthAccount::new(
                account_id.clone(),
                host_id,
                AuthAccountClientId::new("client_id".to_string()),
                EventVersion::new(Uuid::now_v7()),
            );
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
            assert_eq!(result, Some(auth_account.clone()));
            database
                .auth_account_read_model()
                .delete(&mut transaction, auth_account.id())
                .await
                .unwrap();
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn update() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let host_id = AuthHostId::new(Uuid::now_v7());
            let account_id = AuthAccountId::new(Uuid::now_v7());
            let auth_host = AuthHost::new(host_id.clone(), AuthHostUrl::new(Uuid::now_v7()));
            database
                .auth_host_modifier()
                .create(&mut transaction, &auth_host)
                .await
                .unwrap();
            let auth_account = AuthAccount::new(
                account_id.clone(),
                host_id.clone(),
                AuthAccountClientId::new("client_id".to_string()),
                EventVersion::new(Uuid::now_v7()),
            );
            database
                .auth_account_read_model()
                .create(&mut transaction, &auth_account)
                .await
                .unwrap();
            let updated_auth_account = AuthAccount::new(
                account_id.clone(),
                host_id,
                AuthAccountClientId::new("updated_client_id".to_string()),
                EventVersion::new(Uuid::now_v7()),
            );
            database
                .auth_account_read_model()
                .update(&mut transaction, &updated_auth_account)
                .await
                .unwrap();
            let result = database
                .auth_account_read_model()
                .find_by_id(&mut transaction, &account_id)
                .await
                .unwrap();
            assert_eq!(result, Some(updated_auth_account));
            database
                .auth_account_read_model()
                .delete(&mut transaction, auth_account.id())
                .await
                .unwrap();
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn delete() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let host_id = AuthHostId::new(Uuid::now_v7());
            let auth_host = AuthHost::new(host_id.clone(), AuthHostUrl::new(Uuid::now_v7()));
            database
                .auth_host_modifier()
                .create(&mut transaction, &auth_host)
                .await
                .unwrap();
            let account_id = AuthAccountId::new(Uuid::now_v7());
            let auth_account = AuthAccount::new(
                account_id.clone(),
                host_id,
                AuthAccountClientId::new("client_id".to_string()),
                EventVersion::new(Uuid::now_v7()),
            );
            database
                .auth_account_read_model()
                .create(&mut transaction, &auth_account)
                .await
                .unwrap();
            database
                .auth_account_read_model()
                .delete(&mut transaction, &account_id)
                .await
                .unwrap();
            let result = database
                .auth_account_read_model()
                .find_by_id(&mut transaction, &account_id)
                .await
                .unwrap();
            assert_eq!(result, None);
        }
    }
}
