use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use kernel::interfaces::modify::{DependOnStellarAccountModifier, StellarAccountModifier};
use kernel::interfaces::query::{DependOnStellarAccountQuery, StellarAccountQuery};
use kernel::prelude::entity::{
    EventVersion, StellarAccount, StellarAccountAccessToken, StellarAccountClientId,
    StellarAccountId, StellarAccountRefreshToken, StellarHostId,
};
use kernel::KernelError;
use sqlx::PgConnection;
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct StellarAccountRow {
    id: Uuid,
    host_id: Uuid,
    client_id: String,
    access_token: String,
    refresh_token: String,
    version: Uuid,
}

impl From<StellarAccountRow> for StellarAccount {
    fn from(value: StellarAccountRow) -> Self {
        StellarAccount::new(
            StellarAccountId::new(value.id),
            StellarHostId::new(value.host_id),
            StellarAccountClientId::new(value.client_id),
            StellarAccountAccessToken::new(value.access_token),
            StellarAccountRefreshToken::new(value.refresh_token),
            EventVersion::new(value.version),
        )
    }
}

pub struct PostgresStellarAccountRepository;

impl StellarAccountQuery for PostgresStellarAccountRepository {
    type Transaction = PostgresConnection;

    async fn find_by_id(
        &self,
        transaction: &mut Self::Transaction,
        account_id: &StellarAccountId,
    ) -> error_stack::Result<Option<StellarAccount>, KernelError> {
        let con: &mut PgConnection = transaction;
        sqlx::query_as::<_, StellarAccountRow>(
            //language=postgresql
            r#"
            SELECT id, host_id, client_id, access_token, refresh_token, version
            FROM stellar_accounts
            WHERE id = $1
            "#,
        )
        .bind(account_id.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()
        .map(|option| option.map(|row| row.into()))
    }
}

impl DependOnStellarAccountQuery for PostgresDatabase {
    type StellarAccountQuery = PostgresStellarAccountRepository;

    fn stellar_account_query(&self) -> &Self::StellarAccountQuery {
        &PostgresStellarAccountRepository
    }
}

impl StellarAccountModifier for PostgresStellarAccountRepository {
    type Transaction = PostgresConnection;

    async fn create(
        &self,
        transaction: &mut Self::Transaction,
        stellar_account: &StellarAccount,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = transaction;
        sqlx::query(
            //language=postgresql
            r#"
            INSERT INTO stellar_accounts (id, host_id, client_id, access_token, refresh_token, version) VALUES ($1, $2, $3, $4, $5, $6)
            "#
        )
            .bind(stellar_account.id().as_ref())
            .bind(stellar_account.host().as_ref())
            .bind(stellar_account.client_id().as_ref())
            .bind(stellar_account.access_token().as_ref())
            .bind(stellar_account.refresh_token().as_ref())
            .bind(stellar_account.version().as_ref())
            .execute(con)
            .await
            .convert_error()?;
        Ok(())
    }

    async fn update(
        &self,
        transaction: &mut Self::Transaction,
        stellar_account: &StellarAccount,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = transaction;
        sqlx::query(
            //language=postgresql
            r#"
            UPDATE stellar_accounts SET host_id = $2, client_id = $3, access_token = $4, refresh_token = $5, version = $6
            WHERE id = $1
            "#
        )
            .bind(stellar_account.id().as_ref())
            .bind(stellar_account.host().as_ref())
            .bind(stellar_account.client_id().as_ref())
            .bind(stellar_account.access_token().as_ref())
            .bind(stellar_account.refresh_token().as_ref())
            .bind(stellar_account.version().as_ref())
            .execute(con)
            .await
            .convert_error()?;
        Ok(())
    }

    async fn delete(
        &self,
        transaction: &mut Self::Transaction,
        account_id: &StellarAccountId,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = transaction;
        sqlx::query(
            //language=postgresql
            r#"
            DELETE FROM stellar_accounts WHERE id = $1
            "#,
        )
        .bind(account_id.as_ref())
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }
}

impl DependOnStellarAccountModifier for PostgresDatabase {
    type StellarAccountModifier = PostgresStellarAccountRepository;

    fn stellar_account_modifier(&self) -> &Self::StellarAccountModifier {
        &PostgresStellarAccountRepository
    }
}

#[cfg(test)]
mod test {
    mod query {
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::modify::{
            DependOnStellarAccountModifier, DependOnStellarHostModifier, StellarAccountModifier,
            StellarHostModifier,
        };
        use kernel::interfaces::query::{DependOnStellarAccountQuery, StellarAccountQuery};
        use kernel::prelude::entity::{
            EventVersion, StellarAccount, StellarAccountAccessToken, StellarAccountClientId,
            StellarAccountId, StellarAccountRefreshToken, StellarHost, StellarHostId,
            StellarHostUrl,
        };
        use uuid::Uuid;

        #[tokio::test]
        async fn find_by_id() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let stellar_host_id = StellarHostId::new(Uuid::now_v7());
            let stellar_host =
                StellarHost::new(stellar_host_id.clone(), StellarHostUrl::new(Uuid::now_v7()));
            database
                .stellar_host_modifier()
                .create(&mut transaction, &stellar_host)
                .await
                .unwrap();
            let account_id = StellarAccountId::new(Uuid::now_v7());
            let stellar_account = StellarAccount::new(
                account_id.clone(),
                stellar_host_id,
                StellarAccountClientId::new("client_id".to_string()),
                StellarAccountAccessToken::new("access_token".to_string()),
                StellarAccountRefreshToken::new("refresh_token".to_string()),
                EventVersion::new(Uuid::now_v7()),
            );

            database
                .stellar_account_modifier()
                .create(&mut transaction, &stellar_account)
                .await
                .unwrap();
            let result = database
                .stellar_account_query()
                .find_by_id(&mut transaction, &account_id)
                .await
                .unwrap();
            assert_eq!(result, Some(stellar_account.clone()));
            database
                .stellar_account_modifier()
                .delete(&mut transaction, stellar_account.id())
                .await
                .unwrap();
        }
    }

    mod modify {
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::modify::{
            DependOnStellarAccountModifier, DependOnStellarHostModifier, StellarAccountModifier,
            StellarHostModifier,
        };
        use kernel::interfaces::query::{DependOnStellarAccountQuery, StellarAccountQuery};
        use kernel::prelude::entity::{
            EventVersion, StellarAccount, StellarAccountAccessToken, StellarAccountClientId,
            StellarAccountId, StellarAccountRefreshToken, StellarHost, StellarHostId,
            StellarHostUrl,
        };
        use uuid::Uuid;

        #[tokio::test]
        async fn create() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let host_id = StellarHostId::new(Uuid::now_v7());
            let account_id = StellarAccountId::new(Uuid::now_v7());
            let stellar_host =
                StellarHost::new(host_id.clone(), StellarHostUrl::new(Uuid::now_v7()));
            database
                .stellar_host_modifier()
                .create(&mut transaction, &stellar_host)
                .await
                .unwrap();
            let stellar_account = StellarAccount::new(
                account_id.clone(),
                host_id,
                StellarAccountClientId::new("client_id".to_string()),
                StellarAccountAccessToken::new("access_token".to_string()),
                StellarAccountRefreshToken::new("refresh_token".to_string()),
                EventVersion::new(Uuid::now_v7()),
            );
            database
                .stellar_account_modifier()
                .create(&mut transaction, &stellar_account)
                .await
                .unwrap();
            let result = database
                .stellar_account_query()
                .find_by_id(&mut transaction, &account_id)
                .await
                .unwrap();
            assert_eq!(result, Some(stellar_account.clone()));
            database
                .stellar_account_modifier()
                .delete(&mut transaction, stellar_account.id())
                .await
                .unwrap();
        }

        #[tokio::test]
        async fn update() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let host_id = StellarHostId::new(Uuid::now_v7());
            let account_id = StellarAccountId::new(Uuid::now_v7());
            let stellar_host =
                StellarHost::new(host_id.clone(), StellarHostUrl::new(Uuid::now_v7()));
            database
                .stellar_host_modifier()
                .create(&mut transaction, &stellar_host)
                .await
                .unwrap();
            let stellar_account = StellarAccount::new(
                account_id.clone(),
                host_id.clone(),
                StellarAccountClientId::new("client_id".to_string()),
                StellarAccountAccessToken::new("access_token".to_string()),
                StellarAccountRefreshToken::new("refresh_token".to_string()),
                EventVersion::new(Uuid::now_v7()),
            );
            database
                .stellar_account_modifier()
                .create(&mut transaction, &stellar_account)
                .await
                .unwrap();
            let updated_stellar_account = StellarAccount::new(
                account_id.clone(),
                host_id,
                StellarAccountClientId::new("updated_client_id".to_string()),
                StellarAccountAccessToken::new("updated_access_token".to_string()),
                StellarAccountRefreshToken::new("updated_refresh_token".to_string()),
                EventVersion::new(Uuid::now_v7()),
            );
            database
                .stellar_account_modifier()
                .update(&mut transaction, &updated_stellar_account)
                .await
                .unwrap();
            let result = database
                .stellar_account_query()
                .find_by_id(&mut transaction, &account_id)
                .await
                .unwrap();
            assert_eq!(result, Some(updated_stellar_account));
            database
                .stellar_account_modifier()
                .delete(&mut transaction, stellar_account.id())
                .await
                .unwrap();
        }

        #[tokio::test]
        async fn delete() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let host_id = StellarHostId::new(Uuid::now_v7());
            let stellar_host =
                StellarHost::new(host_id.clone(), StellarHostUrl::new(Uuid::now_v7()));
            database
                .stellar_host_modifier()
                .create(&mut transaction, &stellar_host)
                .await
                .unwrap();
            let account_id = StellarAccountId::new(Uuid::now_v7());
            let stellar_account = StellarAccount::new(
                account_id.clone(),
                host_id,
                StellarAccountClientId::new("client_id".to_string()),
                StellarAccountAccessToken::new("access_token".to_string()),
                StellarAccountRefreshToken::new("refresh_token".to_string()),
                EventVersion::new(Uuid::now_v7()),
            );
            database
                .stellar_account_modifier()
                .create(&mut transaction, &stellar_account)
                .await
                .unwrap();
            database
                .stellar_account_modifier()
                .delete(&mut transaction, &account_id)
                .await
                .unwrap();
            let result = database
                .stellar_account_query()
                .find_by_id(&mut transaction, &account_id)
                .await
                .unwrap();
            assert_eq!(result, None);
            database
                .stellar_account_modifier()
                .delete(&mut transaction, stellar_account.id())
                .await
                .unwrap();
        }
    }
}
