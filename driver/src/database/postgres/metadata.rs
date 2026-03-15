use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use kernel::interfaces::read_model::{DependOnMetadataReadModel, MetadataReadModel};
use kernel::prelude::entity::{
    AccountId, EventVersion, Metadata, MetadataContent, MetadataId, MetadataLabel, Nanoid,
};
use kernel::KernelError;
use sqlx::PgConnection;

#[derive(sqlx::FromRow)]
struct MetadataRow {
    id: i64,
    account_id: i64,
    label: String,
    content: String,
    version: i64,
    nanoid: String,
}

impl From<MetadataRow> for Metadata {
    fn from(row: MetadataRow) -> Self {
        Metadata::new(
            MetadataId::new(row.id),
            AccountId::new(row.account_id),
            MetadataLabel::new(row.label),
            MetadataContent::new(row.content),
            EventVersion::new(row.version),
            Nanoid::new(row.nanoid),
        )
    }
}

pub struct PostgresMetadataReadModel;

impl MetadataReadModel for PostgresMetadataReadModel {
    type Executor = PostgresConnection;

    async fn find_by_id(
        &self,
        executor: &mut Self::Executor,
        id: &MetadataId,
    ) -> error_stack::Result<Option<Metadata>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, MetadataRow>(
            // language=postgresql
            r#"
            SELECT id, account_id, label, content, version, nanoid
            FROM metadatas
            WHERE id = $1
            "#,
        )
        .bind(id.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()
        .map(|row| row.map(|row| row.into()))
    }

    async fn find_by_account_id(
        &self,
        executor: &mut Self::Executor,
        account_id: &AccountId,
    ) -> error_stack::Result<Vec<Metadata>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, MetadataRow>(
            // language=postgresql
            r#"
            SELECT id, account_id, label, content, version, nanoid
            FROM metadatas
            WHERE account_id = $1
            "#,
        )
        .bind(account_id.as_ref())
        .fetch_all(con)
        .await
        .convert_error()
        .map(|rows| rows.into_iter().map(|row| row.into()).collect())
    }

    async fn find_by_account_ids(
        &self,
        executor: &mut Self::Executor,
        account_ids: &[AccountId],
    ) -> error_stack::Result<Vec<Metadata>, KernelError> {
        let con: &mut PgConnection = executor;
        let ids: Vec<i64> = account_ids.iter().map(|id| *id.as_ref()).collect();
        sqlx::query_as::<_, MetadataRow>(
            // language=postgresql
            r#"
            SELECT id, account_id, label, content, version, nanoid
            FROM metadatas
            WHERE account_id = ANY($1)
            "#,
        )
        .bind(&ids)
        .fetch_all(con)
        .await
        .convert_error()
        .map(|rows| rows.into_iter().map(|row| row.into()).collect())
    }

    async fn create(
        &self,
        executor: &mut Self::Executor,
        metadata: &Metadata,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            // language=postgresql
            r#"
            INSERT INTO metadatas (id, account_id, label, content, version, nanoid)
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(metadata.id().as_ref())
        .bind(metadata.account_id().as_ref())
        .bind(metadata.label().as_ref())
        .bind(metadata.content().as_ref())
        .bind(metadata.version().as_ref())
        .bind(metadata.nanoid().as_ref())
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }

    async fn update(
        &self,
        executor: &mut Self::Executor,
        metadata: &Metadata,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            // language=postgresql
            r#"
            UPDATE metadatas
            SET label = $1, content = $2, version = $3
            WHERE id = $4
            "#,
        )
        .bind(metadata.label().as_ref())
        .bind(metadata.content().as_ref())
        .bind(metadata.version().as_ref())
        .bind(metadata.id().as_ref())
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }

    async fn delete(
        &self,
        executor: &mut Self::Executor,
        metadata_id: &MetadataId,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            // language=postgresql
            r#"
            DELETE FROM metadatas
            WHERE id = $1
            "#,
        )
        .bind(metadata_id.as_ref())
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }
}

impl DependOnMetadataReadModel for PostgresDatabase {
    type MetadataReadModel = PostgresMetadataReadModel;

    fn metadata_read_model(&self) -> &Self::MetadataReadModel {
        &PostgresMetadataReadModel
    }
}

#[cfg(test)]
mod test {
    mod read_model {
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::read_model::{
            AccountReadModel, DependOnAccountReadModel, DependOnMetadataReadModel,
            MetadataReadModel,
        };
        use kernel::prelude::entity::{AccountId, Metadata, MetadataId};
        use kernel::test_utils::{AccountBuilder, MetadataBuilder};

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_id() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let account_id = AccountId::default();
            let account = AccountBuilder::new().id(account_id.clone()).build();
            let metadata_id = MetadataId::new(kernel::generate_id());
            let metadata = MetadataBuilder::new()
                .id(metadata_id.clone())
                .account_id(account_id.clone())
                .build();

            database
                .account_read_model()
                .create(&mut transaction, &account)
                .await
                .unwrap();
            database
                .metadata_read_model()
                .create(&mut transaction, &metadata)
                .await
                .unwrap();

            let found = database
                .metadata_read_model()
                .find_by_id(&mut transaction, &metadata_id)
                .await
                .unwrap();
            assert_eq!(found.as_ref().map(Metadata::id), Some(metadata.id()));

            // Non-existent id returns None
            let not_found = database
                .metadata_read_model()
                .find_by_id(&mut transaction, &MetadataId::new(kernel::generate_id()))
                .await
                .unwrap();
            assert!(not_found.is_none());

            database
                .account_read_model()
                .deactivate(&mut transaction, account.id())
                .await
                .unwrap();
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_account_id() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let account_id = AccountId::default();
            let account = AccountBuilder::new().id(account_id.clone()).build();

            let metadata1 = MetadataBuilder::new()
                .account_id(account_id.clone())
                .build();
            let metadata2 = MetadataBuilder::new()
                .account_id(account_id.clone())
                .label("label2")
                .content("content2")
                .build();

            database
                .account_read_model()
                .create(&mut transaction, &account)
                .await
                .unwrap();
            database
                .metadata_read_model()
                .create(&mut transaction, &metadata1)
                .await
                .unwrap();
            database
                .metadata_read_model()
                .create(&mut transaction, &metadata2)
                .await
                .unwrap();

            let found = database
                .metadata_read_model()
                .find_by_account_id(&mut transaction, &account_id)
                .await
                .unwrap();
            assert_eq!(found.len(), 2);
            let ids: Vec<_> = found.iter().map(Metadata::id).collect();
            assert!(ids.contains(&metadata1.id()));
            assert!(ids.contains(&metadata2.id()));

            // Non-existent account_id returns empty vec
            let not_found = database
                .metadata_read_model()
                .find_by_account_id(&mut transaction, &AccountId::default())
                .await
                .unwrap();
            assert!(not_found.is_empty());

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

            let account_id = AccountId::default();
            let account = AccountBuilder::new().id(account_id.clone()).build();
            let metadata_id = MetadataId::new(kernel::generate_id());
            let metadata = MetadataBuilder::new()
                .id(metadata_id.clone())
                .account_id(account_id.clone())
                .build();

            database
                .account_read_model()
                .create(&mut transaction, &account)
                .await
                .unwrap();
            database
                .metadata_read_model()
                .create(&mut transaction, &metadata)
                .await
                .unwrap();

            let found = database
                .metadata_read_model()
                .find_by_id(&mut transaction, &metadata_id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(found.id(), metadata.id());
            assert_eq!(found.label(), metadata.label());
            assert_eq!(found.content(), metadata.content());

            database
                .account_read_model()
                .deactivate(&mut transaction, account.id())
                .await
                .unwrap();
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn update() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let account_id = AccountId::default();
            let account = AccountBuilder::new().id(account_id.clone()).build();
            let metadata_id = MetadataId::new(kernel::generate_id());
            let metadata = MetadataBuilder::new()
                .id(metadata_id.clone())
                .account_id(account_id.clone())
                .build();

            database
                .account_read_model()
                .create(&mut transaction, &account)
                .await
                .unwrap();
            database
                .metadata_read_model()
                .create(&mut transaction, &metadata)
                .await
                .unwrap();

            let updated_metadata = MetadataBuilder::new()
                .id(metadata_id.clone())
                .account_id(account_id.clone())
                .label("updated_label")
                .content("updated_content")
                .build();
            database
                .metadata_read_model()
                .update(&mut transaction, &updated_metadata)
                .await
                .unwrap();

            let found = database
                .metadata_read_model()
                .find_by_id(&mut transaction, &metadata_id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(found.id(), updated_metadata.id());
            assert_eq!(found.label(), updated_metadata.label());
            assert_eq!(found.content(), updated_metadata.content());

            database
                .account_read_model()
                .deactivate(&mut transaction, account.id())
                .await
                .unwrap();
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn delete() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let account_id = AccountId::default();
            let account = AccountBuilder::new().id(account_id.clone()).build();
            let metadata_id = MetadataId::new(kernel::generate_id());
            let metadata = MetadataBuilder::new()
                .id(metadata_id.clone())
                .account_id(account_id.clone())
                .build();

            database
                .account_read_model()
                .create(&mut transaction, &account)
                .await
                .unwrap();
            database
                .metadata_read_model()
                .create(&mut transaction, &metadata)
                .await
                .unwrap();

            database
                .metadata_read_model()
                .delete(&mut transaction, &metadata_id)
                .await
                .unwrap();

            let found = database
                .metadata_read_model()
                .find_by_id(&mut transaction, &metadata_id)
                .await
                .unwrap();
            assert!(found.is_none());

            database
                .account_read_model()
                .deactivate(&mut transaction, account.id())
                .await
                .unwrap();
        }
    }
}
