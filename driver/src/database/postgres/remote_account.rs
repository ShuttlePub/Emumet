use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use kernel::interfaces::modify::{DependOnRemoteAccountModifier, RemoteAccountModifier};
use kernel::interfaces::query::{DependOnRemoteAccountQuery, RemoteAccountQuery};
use kernel::prelude::entity::{
    ImageId, RemoteAccount, RemoteAccountAcct, RemoteAccountId, RemoteAccountUrl,
};
use kernel::KernelError;
use sqlx::PgConnection;
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct RemoteAccountRow {
    id: Uuid,
    acct: String,
    url: String,
    icon_id: Option<Uuid>,
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

impl RemoteAccountQuery for PostgresRemoteAccountRepository {
    type Transaction = PostgresConnection;

    async fn find_by_id(
        &self,
        transaction: &mut Self::Transaction,
        id: &RemoteAccountId,
    ) -> error_stack::Result<Option<RemoteAccount>, KernelError> {
        let con: &mut PgConnection = transaction;
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
        transaction: &mut Self::Transaction,
        acct: &RemoteAccountAcct,
    ) -> error_stack::Result<Option<RemoteAccount>, KernelError> {
        let con: &mut PgConnection = transaction;
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
        transaction: &mut Self::Transaction,
        url: &RemoteAccountUrl,
    ) -> error_stack::Result<Option<RemoteAccount>, KernelError> {
        let con: &mut PgConnection = transaction;
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
}

impl DependOnRemoteAccountQuery for PostgresDatabase {
    type RemoteAccountQuery = PostgresRemoteAccountRepository;

    fn remote_account_query(&self) -> &Self::RemoteAccountQuery {
        &PostgresRemoteAccountRepository
    }
}

impl RemoteAccountModifier for PostgresRemoteAccountRepository {
    type Transaction = PostgresConnection;

    async fn create(
        &self,
        transaction: &mut Self::Transaction,
        account: &RemoteAccount,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = transaction;
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
        transaction: &mut Self::Transaction,
        account: &RemoteAccount,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = transaction;
        sqlx::query(
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
        Ok(())
    }

    async fn delete(
        &self,
        transaction: &mut Self::Transaction,
        account_id: &RemoteAccountId,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = transaction;
        sqlx::query(
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
        Ok(())
    }
}

impl DependOnRemoteAccountModifier for PostgresDatabase {
    type RemoteAccountModifier = PostgresRemoteAccountRepository;

    fn remote_account_modifier(&self) -> &Self::RemoteAccountModifier {
        &PostgresRemoteAccountRepository
    }
}

#[cfg(test)]
mod test {
    use kernel::prelude::entity::{RemoteAccountAcct, RemoteAccountUrl};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn acct_url(name: Option<&str>) -> (RemoteAccountAcct, RemoteAccountUrl) {
        if let Some(name) = name {
            (
                RemoteAccountAcct(format!("{}@example.com", name)),
                RemoteAccountUrl(format!("https://example.com/users/{}", name)),
            )
        } else {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let c = COUNTER.fetch_add(1, Ordering::Relaxed);
            (
                RemoteAccountAcct(format!("example{}@example.com", c)),
                RemoteAccountUrl(format!("https://example.com/users/example{}", c)),
            )
        }
    }

    mod query {
        use crate::database::postgres::remote_account::test::acct_url;
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::modify::{DependOnRemoteAccountModifier, RemoteAccountModifier};
        use kernel::interfaces::query::{DependOnRemoteAccountQuery, RemoteAccountQuery};
        use kernel::prelude::entity::{RemoteAccount, RemoteAccountId};
        use uuid::Uuid;

        #[tokio::test]
        async fn find_by_id() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let id = RemoteAccountId(Uuid::now_v7());
            let (acct, url) = acct_url(None);
            let remote_account = RemoteAccount::new(id, acct, url, None);
            database
                .remote_account_modifier()
                .create(&mut transaction, &remote_account)
                .await
                .unwrap();
            let result = database
                .remote_account_query()
                .find_by_id(&mut transaction, &id)
                .await
                .unwrap();
            assert_eq!(result, Some(remote_account));
        }

        #[tokio::test]
        async fn find_by_acct() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let (acct, url) = acct_url(None);
            let remote_account =
                RemoteAccount::new(RemoteAccountId(Uuid::now_v7()), acct, url, None);
            database
                .remote_account_modifier()
                .create(&mut transaction, &remote_account)
                .await
                .unwrap();
            let result = database
                .remote_account_query()
                .find_by_acct(&mut transaction, &acct)
                .await
                .unwrap();
            assert_eq!(result, Some(remote_account));
        }

        #[tokio::test]
        async fn find_by_url() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let (acct, url) = acct_url(None);
            let remote_account =
                RemoteAccount::new(RemoteAccountId(Uuid::now_v7()), acct, url, None);
            database
                .remote_account_modifier()
                .create(&mut transaction, &remote_account)
                .await
                .unwrap();
            let result = database
                .remote_account_query()
                .find_by_url(&mut transaction, &url)
                .await
                .unwrap();
            assert_eq!(result, Some(remote_account));
        }
    }

    mod modify {
        use crate::database::postgres::remote_account::test::acct_url;
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::modify::{DependOnRemoteAccountModifier, RemoteAccountModifier};
        use kernel::interfaces::query::{DependOnRemoteAccountQuery, RemoteAccountQuery};
        use kernel::prelude::entity::{RemoteAccount, RemoteAccountId};
        use uuid::Uuid;

        #[tokio::test]
        async fn create() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let id = RemoteAccountId(Uuid::now_v7());
            let (acct, url) = acct_url(None);
            let remote_account = RemoteAccount::new(id, acct, url, None);
            database
                .remote_account_modifier()
                .create(&mut transaction, &remote_account)
                .await
                .unwrap();
        }

        #[tokio::test]
        async fn update() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let id = RemoteAccountId(Uuid::now_v7());
            let (acct, url) = acct_url(None);
            let remote_account = RemoteAccount::new(id.clone(), acct, url, None);
            database
                .remote_account_modifier()
                .create(&mut transaction, &remote_account)
                .await
                .unwrap();

            let (acct, url) = acct_url(None);
            let remote_account = RemoteAccount::new(id.clone(), acct, url, None);
            database
                .remote_account_modifier()
                .update(&mut transaction, &remote_account)
                .await
                .unwrap();
            let result = database
                .remote_account_query()
                .find_by_id(&mut transaction, &id)
                .await
                .unwrap();
            assert_eq!(result, Some(remote_account));
        }

        #[tokio::test]
        async fn delete() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let id = RemoteAccountId(Uuid::now_v7());
            let (acct, url) = acct_url(None);
            let remote_account = RemoteAccount::new(id.clone(), acct, url, None);
            database
                .remote_account_modifier()
                .create(&mut transaction, &remote_account)
                .await
                .unwrap();

            database
                .remote_account_modifier()
                .delete(&mut transaction, &id)
                .await
                .unwrap();
            let result = database
                .remote_account_query()
                .find_by_id(&mut transaction, &id)
                .await
                .unwrap();
            assert_eq!(result, None);
        }
    }
}
