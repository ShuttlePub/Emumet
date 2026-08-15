use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use error_stack::Report;
use kernel::interfaces::read_model::{
    DependOnMetadataReadModel, MetadataProjection, MetadataReadModel,
};
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

impl From<MetadataRow> for MetadataProjection {
    fn from(row: MetadataRow) -> Self {
        MetadataProjection::new(
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
    type Connection = PostgresConnection;

    async fn find_by_id(
        &self,
        executor: &mut Self::Connection,
        id: &MetadataId,
    ) -> error_stack::Result<Option<MetadataProjection>, KernelError> {
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
        .map(|option| option.map(MetadataProjection::from))
    }

    async fn find_by_account_id(
        &self,
        executor: &mut Self::Connection,
        account_id: &AccountId,
    ) -> error_stack::Result<Vec<MetadataProjection>, KernelError> {
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
        .map(|rows| rows.into_iter().map(MetadataProjection::from).collect())
    }

    async fn find_by_account_ids(
        &self,
        executor: &mut Self::Connection,
        account_ids: &[AccountId],
    ) -> error_stack::Result<Vec<MetadataProjection>, KernelError> {
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
        .map(|rows| rows.into_iter().map(MetadataProjection::from).collect())
    }

    async fn create(
        &self,
        executor: &mut Self::Connection,
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
        executor: &mut Self::Connection,
        metadata: &Metadata,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        let result = sqlx::query(
            // language=postgresql
            r#"
            UPDATE metadatas
            SET label = $2, content = $3, version = $4
            WHERE id = $1
            "#,
        )
        .bind(metadata.id().as_ref())
        .bind(metadata.label().as_ref())
        .bind(metadata.content().as_ref())
        .bind(metadata.version().as_ref())
        .execute(con)
        .await
        .convert_error()?;
        if result.rows_affected() == 0 {
            return Err(Report::new(KernelError::NotFound)
                .attach_printable("Target metadata not found for update"));
        }
        Ok(())
    }

    async fn delete(
        &self,
        executor: &mut Self::Connection,
        metadata_id: &MetadataId,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        let result = sqlx::query(
            // language=postgresql
            r#"
            DELETE FROM metadatas WHERE id = $1
            "#,
        )
        .bind(metadata_id.as_ref())
        .execute(con)
        .await
        .convert_error()?;
        if result.rows_affected() == 0 {
            return Err(Report::new(KernelError::NotFound)
                .attach_printable("Target metadata not found for delete"));
        }
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
        use kernel::prelude::entity::EventVersion;
        use kernel::prelude::entity::{AccountId, MetadataId};
        use kernel::test_utils::{AccountBuilder, MetadataBuilder};

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_id() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut conn = database.connection().await.unwrap();

            let account_id = AccountId::default();
            let account = AccountBuilder::new().id(account_id.clone()).build();
            let metadata_id = MetadataId::new(kernel::generate_id());
            let metadata = MetadataBuilder::new()
                .id(metadata_id.clone())
                .account_id(account_id.clone())
                .build();

            database
                .account_read_model()
                .create(&mut conn, &account)
                .await
                .unwrap();
            database
                .metadata_read_model()
                .create(&mut conn, &metadata)
                .await
                .unwrap();

            let result = database
                .metadata_read_model()
                .find_by_id(&mut conn, &metadata_id)
                .await
                .unwrap();
            assert!(result.is_some());
            let result = result.unwrap();
            assert_eq!(result.id(), &metadata_id);
            assert_eq!(result.label(), metadata.label());
            assert_eq!(result.content(), metadata.content());

            database
                .account_read_model()
                .deactivate(&mut conn, account.id())
                .await
                .unwrap();
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_account_id() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut conn = database.connection().await.unwrap();

            let account_id = AccountId::default();
            let account = AccountBuilder::new().id(account_id.clone()).build();
            let metadata_id = MetadataId::new(kernel::generate_id());
            let metadata = MetadataBuilder::new()
                .id(metadata_id.clone())
                .account_id(account_id.clone())
                .build();

            database
                .account_read_model()
                .create(&mut conn, &account)
                .await
                .unwrap();
            database
                .metadata_read_model()
                .create(&mut conn, &metadata)
                .await
                .unwrap();

            let result = database
                .metadata_read_model()
                .find_by_account_id(&mut conn, &account_id)
                .await
                .unwrap();
            assert_eq!(result.len(), 1);
            assert_eq!(result[0].id(), &metadata_id);

            database
                .account_read_model()
                .deactivate(&mut conn, account.id())
                .await
                .unwrap();
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn update_and_find_by_id() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut conn = database.connection().await.unwrap();

            let account_id = AccountId::default();
            let account = AccountBuilder::new().id(account_id.clone()).build();
            let metadata_id = MetadataId::new(kernel::generate_id());
            let metadata = MetadataBuilder::new()
                .id(metadata_id.clone())
                .account_id(account_id.clone())
                .label("label")
                .content("content")
                .build();
            let updated_metadata = MetadataBuilder::new()
                .id(metadata_id.clone())
                .account_id(account_id.clone())
                .label("updated")
                .content("updated content")
                .version(EventVersion::new(2))
                .build();

            database
                .account_read_model()
                .create(&mut conn, &account)
                .await
                .unwrap();
            database
                .metadata_read_model()
                .create(&mut conn, &metadata)
                .await
                .unwrap();
            database
                .metadata_read_model()
                .update(&mut conn, &updated_metadata)
                .await
                .unwrap();

            let found = database
                .metadata_read_model()
                .find_by_id(&mut conn, &metadata_id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(found.id(), updated_metadata.id());
            assert_eq!(found.label(), updated_metadata.label());
            assert_eq!(found.content(), updated_metadata.content());

            database
                .account_read_model()
                .deactivate(&mut conn, account.id())
                .await
                .unwrap();
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn delete() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut conn = database.connection().await.unwrap();

            let account_id = AccountId::default();
            let account = AccountBuilder::new().id(account_id.clone()).build();
            let metadata_id = MetadataId::new(kernel::generate_id());
            let metadata = MetadataBuilder::new()
                .id(metadata_id.clone())
                .account_id(account_id.clone())
                .build();

            database
                .account_read_model()
                .create(&mut conn, &account)
                .await
                .unwrap();
            database
                .metadata_read_model()
                .create(&mut conn, &metadata)
                .await
                .unwrap();

            database
                .metadata_read_model()
                .delete(&mut conn, &metadata_id)
                .await
                .unwrap();

            let found = database
                .metadata_read_model()
                .find_by_id(&mut conn, &metadata_id)
                .await
                .unwrap();
            assert!(found.is_none());

            database
                .account_read_model()
                .deactivate(&mut conn, account.id())
                .await
                .unwrap();
        }
    }
}
