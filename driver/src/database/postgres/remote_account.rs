use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use error_stack::Report;
use kernel::interfaces::repository::{DependOnRemoteAccountRepository, RemoteAccountRepository};
use kernel::prelude::entity::{
    ImageId, RemoteAccount, RemoteAccountAcct, RemoteAccountId, RemoteAccountUrl,
};
use kernel::KernelError;
use sqlx::PgConnection;

#[derive(sqlx::FromRow)]
struct RemoteAccountRow {
    id: i64,
    acct: String,
    url: String,
    icon_id: Option<i64>,
}

impl From<RemoteAccountRow> for RemoteAccount {
    fn from(row: RemoteAccountRow) -> Self {
        RemoteAccount::new(
            RemoteAccountId::new(row.id),
            RemoteAccountAcct::new(row.acct),
            RemoteAccountUrl::new(row.url),
            row.icon_id.map(ImageId::new),
        )
    }
}

pub struct PostgresRemoteAccountRepository;

impl RemoteAccountRepository for PostgresRemoteAccountRepository {
    type Executor = PostgresConnection;

    async fn find_by_id(
        &self,
        executor: &mut Self::Executor,
        id: &RemoteAccountId,
    ) -> error_stack::Result<Option<RemoteAccount>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, RemoteAccountRow>(
            // language=postgresql
            r#"
            SELECT id, acct, url, icon_id
            FROM remote_accounts
            WHERE id = $1
            "#,
        )
        .bind(id.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()
        .map(|option| option.map(RemoteAccount::from))
    }

    async fn find_by_acct(
        &self,
        executor: &mut Self::Executor,
        acct: &RemoteAccountAcct,
    ) -> error_stack::Result<Option<RemoteAccount>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, RemoteAccountRow>(
            // language=postgresql
            r#"
            SELECT id, acct, url, icon_id
            FROM remote_accounts
            WHERE acct = $1
            "#,
        )
        .bind(acct.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()
        .map(|option| option.map(RemoteAccount::from))
    }

    async fn find_by_url(
        &self,
        executor: &mut Self::Executor,
        url: &RemoteAccountUrl,
    ) -> error_stack::Result<Option<RemoteAccount>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, RemoteAccountRow>(
            // language=postgresql
            r#"
            SELECT id, acct, url, icon_id
            FROM remote_accounts
            WHERE url = $1
            "#,
        )
        .bind(url.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()
        .map(|option| option.map(RemoteAccount::from))
    }

    async fn create(
        &self,
        executor: &mut Self::Executor,
        account: &RemoteAccount,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            // language=postgresql
            r#"
            INSERT INTO remote_accounts (id, acct, url, icon_id)
            VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(account.id().as_ref())
        .bind(account.acct().as_ref())
        .bind(account.url().as_ref())
        .bind(account.icon_id().as_ref().map(ImageId::as_ref))
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }

    async fn update(
        &self,
        executor: &mut Self::Executor,
        account: &RemoteAccount,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        let result = sqlx::query(
            // language=postgresql
            r#"
            UPDATE remote_accounts
            SET acct = $2, url = $3, icon_id = $4
            WHERE id = $1
            "#,
        )
        .bind(account.id().as_ref())
        .bind(account.acct().as_ref())
        .bind(account.url().as_ref())
        .bind(account.icon_id().as_ref().map(ImageId::as_ref))
        .execute(con)
        .await
        .convert_error()?;
        if result.rows_affected() == 0 {
            return Err(Report::new(KernelError::NotFound)
                .attach_printable("Target remote account not found for update"));
        }
        Ok(())
    }

    async fn delete(
        &self,
        executor: &mut Self::Executor,
        account_id: &RemoteAccountId,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        let result = sqlx::query(
            // language=postgresql
            r#"
            DELETE FROM remote_accounts
            WHERE id = $1
            "#,
        )
        .bind(account_id.as_ref())
        .execute(con)
        .await
        .convert_error()?;
        if result.rows_affected() == 0 {
            return Err(Report::new(KernelError::NotFound)
                .attach_printable("Target remote account not found for delete"));
        }
        Ok(())
    }
}

impl DependOnRemoteAccountRepository for PostgresDatabase {
    type RemoteAccountRepository = PostgresRemoteAccountRepository;

    fn remote_account_repository(&self) -> &Self::RemoteAccountRepository {
        &PostgresRemoteAccountRepository
    }
}

#[cfg(test)]
mod test {
    mod query {
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::repository::{
            DependOnRemoteAccountRepository, RemoteAccountRepository,
        };
        use kernel::test_utils::{unique_remote_acct, RemoteAccountBuilder};

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_id() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.get_executor().await.unwrap();

            let remote_account = RemoteAccountBuilder::new().build();
            database
                .remote_account_repository()
                .create(&mut transaction, &remote_account)
                .await
                .unwrap();
            let result = database
                .remote_account_repository()
                .find_by_id(&mut transaction, remote_account.id())
                .await
                .unwrap();
            assert_eq!(result, Some(remote_account.clone()));
            database
                .remote_account_repository()
                .delete(&mut transaction, remote_account.id())
                .await
                .unwrap();
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_acct() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.get_executor().await.unwrap();

            let (acct, url) = unique_remote_acct();
            let remote_account = RemoteAccountBuilder::new()
                .acct(acct.as_ref())
                .url(url.as_ref())
                .build();
            database
                .remote_account_repository()
                .create(&mut transaction, &remote_account)
                .await
                .unwrap();
            let result = database
                .remote_account_repository()
                .find_by_acct(&mut transaction, &acct)
                .await
                .unwrap();
            assert_eq!(result, Some(remote_account.clone()));

            database
                .remote_account_repository()
                .delete(&mut transaction, remote_account.id())
                .await
                .unwrap();
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_url() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.get_executor().await.unwrap();

            let (acct, url) = unique_remote_acct();
            let remote_account = RemoteAccountBuilder::new()
                .acct(acct.as_ref())
                .url(url.as_ref())
                .build();
            database
                .remote_account_repository()
                .create(&mut transaction, &remote_account)
                .await
                .unwrap();
            let result = database
                .remote_account_repository()
                .find_by_url(&mut transaction, &url)
                .await
                .unwrap();
            assert_eq!(result, Some(remote_account.clone()));
            database
                .remote_account_repository()
                .delete(&mut transaction, remote_account.id())
                .await
                .unwrap();
        }
    }

    mod modify {
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::repository::{
            DependOnRemoteAccountRepository, RemoteAccountRepository,
        };
        use kernel::prelude::entity::RemoteAccountId;
        use kernel::test_utils::RemoteAccountBuilder;

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn create() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.get_executor().await.unwrap();

            let remote_account = RemoteAccountBuilder::new().build();
            database
                .remote_account_repository()
                .create(&mut transaction, &remote_account)
                .await
                .unwrap();
            database
                .remote_account_repository()
                .delete(&mut transaction, remote_account.id())
                .await
                .unwrap();
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn update() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.get_executor().await.unwrap();

            let id = RemoteAccountId::new(kernel::generate_id());
            let remote_account = RemoteAccountBuilder::new().id(id.clone()).build();
            database
                .remote_account_repository()
                .create(&mut transaction, &remote_account)
                .await
                .unwrap();

            let remote_account = RemoteAccountBuilder::new().id(id.clone()).build();
            database
                .remote_account_repository()
                .update(&mut transaction, &remote_account)
                .await
                .unwrap();
            let result = database
                .remote_account_repository()
                .find_by_id(&mut transaction, &id)
                .await
                .unwrap();
            assert_eq!(result, Some(remote_account.clone()));
            database
                .remote_account_repository()
                .delete(&mut transaction, remote_account.id())
                .await
                .unwrap();
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn delete() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.get_executor().await.unwrap();

            let remote_account = RemoteAccountBuilder::new().build();
            database
                .remote_account_repository()
                .create(&mut transaction, &remote_account)
                .await
                .unwrap();

            database
                .remote_account_repository()
                .delete(&mut transaction, remote_account.id())
                .await
                .unwrap();
            let result = database
                .remote_account_repository()
                .find_by_id(&mut transaction, remote_account.id())
                .await
                .unwrap();
            assert_eq!(result, None);
        }
    }
}
