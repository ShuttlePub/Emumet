use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use kernel::interfaces::modify::{DependOnStellarHostModifier, StellarHostModifier};
use kernel::interfaces::query::{DependOnStellarHostQuery, StellarHostQuery};
use kernel::prelude::entity::{StellarHost, StellarHostId, StellarHostUrl};
use kernel::KernelError;
use sqlx::PgConnection;
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct StellarHostRow {
    id: Uuid,
    url: String,
}

impl From<StellarHostRow> for StellarHost {
    fn from(row: StellarHostRow) -> Self {
        StellarHost::new(StellarHostId::new(row.id), StellarHostUrl::new(row.url))
    }
}

pub struct PostgresStellarHostRepository;

impl StellarHostQuery for PostgresStellarHostRepository {
    type Transaction = PostgresConnection;
    async fn find_by_id(
        &self,
        transaction: &mut Self::Transaction,
        id: &StellarHostId,
    ) -> error_stack::Result<Option<StellarHost>, KernelError> {
        let con: &mut PgConnection = transaction;
        sqlx::query_as::<_, StellarHostRow>(
            // language=postgresql
            r#"
            SELECT id, url
            FROM stellar_hosts
            WHERE id = $1
            "#,
        )
        .bind(id.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()
        .map(|row| row.map(StellarHost::from))
    }

    async fn find_by_url(
        &self,
        transaction: &mut Self::Transaction,
        domain: &StellarHostUrl,
    ) -> error_stack::Result<Option<StellarHost>, KernelError> {
        let con: &mut PgConnection = transaction;
        sqlx::query_as::<_, StellarHostRow>(
            // language=postgresql
            r#"
            SELECT id, url
            FROM stellar_hosts
            WHERE url = $1
            "#,
        )
        .bind(domain.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()
        .map(|row| row.map(StellarHost::from))
    }
}

impl DependOnStellarHostQuery for PostgresDatabase {
    type StellarHostQuery = PostgresStellarHostRepository;

    fn stellar_host_query(&self) -> &Self::StellarHostQuery {
        &PostgresStellarHostRepository
    }
}

impl StellarHostModifier for PostgresStellarHostRepository {
    type Transaction = PostgresConnection;
    async fn create(
        &self,
        transaction: &mut Self::Transaction,
        stellar_host: &StellarHost,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = transaction;
        sqlx::query(
            // language=postgresql
            r#"
            INSERT INTO stellar_hosts (id, url)
            VALUES ($1, $2)
            "#,
        )
        .bind(stellar_host.id().as_ref())
        .bind(stellar_host.url().as_ref())
        .execute(con)
        .await
        .convert_error()
        .map(|_| ())
    }

    async fn update(
        &self,
        transaction: &mut Self::Transaction,
        stellar_host: &StellarHost,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = transaction;
        sqlx::query(
            // language=postgresql
            r#"
            UPDATE stellar_hosts
            SET url = $2
            WHERE id = $1
            "#,
        )
        .bind(stellar_host.id().as_ref())
        .bind(stellar_host.url().as_ref())
        .execute(con)
        .await
        .convert_error()
        .map(|_| ())
    }
}

impl DependOnStellarHostModifier for PostgresDatabase {
    type StellarHostModifier = PostgresStellarHostRepository;

    fn stellar_host_modifier(&self) -> &Self::StellarHostModifier {
        &PostgresStellarHostRepository
    }
}

#[cfg(test)]
mod test {
    use kernel::prelude::entity::StellarHostUrl;
    use uuid::Uuid;

    fn url() -> StellarHostUrl {
        StellarHostUrl::new(format!("https://{}.example.com", Uuid::now_v7()))
    }

    mod query {
        use crate::database::postgres::stellar_host::test::url;
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::modify::{DependOnStellarHostModifier, StellarHostModifier};
        use kernel::interfaces::query::{DependOnStellarHostQuery, StellarHostQuery};
        use kernel::prelude::entity::{StellarHost, StellarHostId};
        use uuid::Uuid;

        #[tokio::test]
        async fn find_by_id() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let stellar_host = StellarHost::new(StellarHostId::new(Uuid::now_v7()), url());
            database
                .stellar_host_modifier()
                .create(&mut transaction, &stellar_host)
                .await
                .unwrap();

            let found_stellar_host = database
                .stellar_host_query()
                .find_by_id(&mut transaction, stellar_host.id())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(stellar_host, found_stellar_host);
        }

        #[tokio::test]
        async fn find_by_url() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let stellar_host = StellarHost::new(StellarHostId::new(Uuid::now_v7()), url());
            database
                .stellar_host_modifier()
                .create(&mut transaction, &stellar_host)
                .await
                .unwrap();

            let found_stellar_host = database
                .stellar_host_query()
                .find_by_url(&mut transaction, stellar_host.url())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(stellar_host, found_stellar_host);
        }
    }

    mod modify {
        use crate::database::postgres::stellar_host::test::url;
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::modify::{DependOnStellarHostModifier, StellarHostModifier};
        use kernel::interfaces::query::{DependOnStellarHostQuery, StellarHostQuery};
        use kernel::prelude::entity::{StellarHost, StellarHostId};
        use uuid::Uuid;

        #[tokio::test]
        async fn create() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let stellar_host = StellarHost::new(StellarHostId::new(Uuid::now_v7()), url());
            database
                .stellar_host_modifier()
                .create(&mut transaction, &stellar_host)
                .await
                .unwrap();

            let found_stellar_host = database
                .stellar_host_query()
                .find_by_id(&mut transaction, stellar_host.id())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(stellar_host, found_stellar_host);
        }

        #[tokio::test]
        async fn update() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let stellar_host = StellarHost::new(StellarHostId::new(Uuid::now_v7()), url());
            database
                .stellar_host_modifier()
                .create(&mut transaction, &stellar_host)
                .await
                .unwrap();

            let updated_stellar_host = StellarHost::new(stellar_host.id().clone(), url());
            database
                .stellar_host_modifier()
                .update(&mut transaction, &updated_stellar_host)
                .await
                .unwrap();

            let found_stellar_host = database
                .stellar_host_query()
                .find_by_id(&mut transaction, stellar_host.id())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(updated_stellar_host, found_stellar_host);
        }
    }
}
