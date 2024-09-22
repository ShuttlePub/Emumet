use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use kernel::interfaces::modify::{DependOnMetadataModifier, MetadataModifier};
use kernel::interfaces::query::{DependOnMetadataQuery, MetadataQuery};
use kernel::prelude::entity::{
    AccountId, CreatedAt, Metadata, MetadataContent, MetadataId, MetadataLabel,
};
use kernel::KernelError;
use sqlx::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct MetadataRow {
    id: Uuid,
    account_id: Uuid,
    label: String,
    content: String,
    created_at: OffsetDateTime,
}

impl From<MetadataRow> for Metadata {
    fn from(row: MetadataRow) -> Self {
        Metadata::new(
            MetadataId::new(row.id),
            AccountId::new(row.account_id),
            MetadataLabel::new(row.label),
            MetadataContent::new(row.content),
            CreatedAt::new(row.created_at),
        )
    }
}

pub struct PostgresMetadataRepository;

impl MetadataQuery for PostgresMetadataRepository {
    type Transaction = PostgresConnection;

    async fn find_by_id(
        &self,
        transaction: &mut Self::Transaction,
        metadata_id: &MetadataId,
    ) -> error_stack::Result<Option<Metadata>, KernelError> {
        let con: &mut PgConnection = transaction;
        sqlx::query_as::<_, MetadataRow>(
            // language=postgresql
            r#"
            SELECT id, account_id, label, content, created_at
            FROM metadatas
            WHERE id = $1
            "#,
        )
        .bind(metadata_id.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()
        .map(|row| row.map(|row| row.into()))
    }

    async fn find_by_account_id(
        &self,
        transaction: &mut Self::Transaction,
        account_id: &AccountId,
    ) -> error_stack::Result<Vec<Metadata>, KernelError> {
        let con: &mut PgConnection = transaction;
        sqlx::query_as::<_, MetadataRow>(
            // language=postgresql
            r#"
            SELECT id, account_id, label, content, created_at
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
}

impl DependOnMetadataQuery for PostgresDatabase {
    type MetadataQuery = PostgresMetadataRepository;

    fn metadata_query(&self) -> &Self::MetadataQuery {
        &PostgresMetadataRepository
    }
}

impl MetadataModifier for PostgresMetadataRepository {
    type Transaction = PostgresConnection;

    async fn create(
        &self,
        transaction: &mut Self::Transaction,
        metadata: &Metadata,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = transaction;
        sqlx::query(
            // language=postgresql
            r#"
            INSERT INTO metadatas (id, account_id, label, content, created_at)
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(metadata.id().as_ref())
        .bind(metadata.account_id().as_ref())
        .bind(metadata.label().as_ref())
        .bind(metadata.content().as_ref())
        .bind(metadata.created_at().as_ref())
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }

    async fn update(
        &self,
        transaction: &mut Self::Transaction,
        metadata: &Metadata,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = transaction;
        sqlx::query(
            // language=postgresql
            r#"
            UPDATE metadatas
            SET label = $1, content = $2
            WHERE id = $3
            "#,
        )
        .bind(metadata.label().as_ref())
        .bind(metadata.content().as_ref())
        .bind(metadata.id().as_ref())
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }

    async fn delete(
        &self,
        transaction: &mut Self::Transaction,
        metadata_id: &MetadataId,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = transaction;
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

impl DependOnMetadataModifier for PostgresDatabase {
    type MetadataModifier = PostgresMetadataRepository;

    fn metadata_modifier(&self) -> &Self::MetadataModifier {
        &PostgresMetadataRepository
    }
}

#[cfg(test)]
mod test {

    mod query {
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::modify::{DependOnMetadataModifier, MetadataModifier};
        use kernel::interfaces::query::{DependOnMetadataQuery, MetadataQuery};
        use kernel::prelude::entity::{
            AccountId, CreatedAt, Metadata, MetadataContent, MetadataId, MetadataLabel,
        };
        use time::OffsetDateTime;
        use uuid::Uuid;

        #[tokio::test]
        async fn find_by_id() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let account_id = AccountId::new(Uuid::new_v4());
            let metadata = Metadata::new(
                MetadataId::new(Uuid::new_v4()),
                account_id.clone(),
                MetadataLabel::new("label".to_string()),
                MetadataContent::new("content".to_string()),
                CreatedAt::new(OffsetDateTime::now_utc()),
            );

            database
                .metadata_modifier()
                .create(&mut transaction, &metadata)
                .await
                .unwrap();

            let found = database
                .metadata_query()
                .find_by_id(&mut transaction, metadata.id())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(found, metadata);
        }

        #[tokio::test]
        async fn find_by_account_id() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let account_id = AccountId::new(Uuid::new_v4());
            let metadata = Metadata::new(
                MetadataId::new(Uuid::new_v4()),
                account_id.clone(),
                MetadataLabel::new("label".to_string()),
                MetadataContent::new("content".to_string()),
                CreatedAt::new(OffsetDateTime::now_utc()),
            );
            let metadata2 = Metadata::new(
                MetadataId::new(Uuid::new_v4()),
                account_id.clone(),
                MetadataLabel::new("label2".to_string()),
                MetadataContent::new("content2".to_string()),
                CreatedAt::new(OffsetDateTime::now_utc()),
            );

            database
                .metadata_modifier()
                .create(&mut transaction, &metadata)
                .await
                .unwrap();
            database
                .metadata_modifier()
                .create(&mut transaction, &metadata2)
                .await
                .unwrap();

            let found = database
                .metadata_query()
                .find_by_account_id(&mut transaction, &account_id)
                .await
                .unwrap();
            assert_eq!(found, vec![metadata, metadata2]);
        }
    }
    mod modify {
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::modify::{DependOnMetadataModifier, MetadataModifier};
        use kernel::interfaces::query::{DependOnMetadataQuery, MetadataQuery};
        use kernel::prelude::entity::{
            AccountId, CreatedAt, Metadata, MetadataContent, MetadataId, MetadataLabel,
        };
        use time::OffsetDateTime;
        use uuid::Uuid;

        #[tokio::test]
        async fn create() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let account_id = AccountId::new(Uuid::new_v4());
            let metadata = Metadata::new(
                MetadataId::new(Uuid::new_v4()),
                account_id.clone(),
                MetadataLabel::new("label".to_string()),
                MetadataContent::new("content".to_string()),
                CreatedAt::new(OffsetDateTime::now_utc()),
            );

            database
                .metadata_modifier()
                .create(&mut transaction, &metadata)
                .await
                .unwrap();

            let found = database
                .metadata_query()
                .find_by_id(&mut transaction, metadata.id())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(found, metadata);
        }

        #[tokio::test]
        async fn update() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let account_id = AccountId::new(Uuid::new_v4());
            let metadata = Metadata::new(
                MetadataId::new(Uuid::new_v4()),
                account_id.clone(),
                MetadataLabel::new("label".to_string()),
                MetadataContent::new("content".to_string()),
                CreatedAt::new(OffsetDateTime::now_utc()),
            );

            database
                .metadata_modifier()
                .create(&mut transaction, &metadata)
                .await
                .unwrap();

            let updated_metadata = Metadata::new(
                metadata.id().clone(),
                account_id.clone(),
                MetadataLabel::new("label2".to_string()),
                MetadataContent::new("content2".to_string()),
                CreatedAt::new(OffsetDateTime::now_utc()),
            );

            database
                .metadata_modifier()
                .update(&mut transaction, &updated_metadata)
                .await
                .unwrap();

            let found = database
                .metadata_query()
                .find_by_id(&mut transaction, metadata.id())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(found, updated_metadata);
        }

        #[tokio::test]
        async fn delete() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let account_id = AccountId::new(Uuid::new_v4());
            let metadata = Metadata::new(
                MetadataId::new(Uuid::new_v4()),
                account_id.clone(),
                MetadataLabel::new("label".to_string()),
                MetadataContent::new("content".to_string()),
                CreatedAt::new(OffsetDateTime::now_utc()),
            );

            database
                .metadata_modifier()
                .create(&mut transaction, &metadata)
                .await
                .unwrap();
            database
                .metadata_modifier()
                .delete(&mut transaction, metadata.id())
                .await
                .unwrap();

            let found = database
                .metadata_query()
                .find_by_id(&mut transaction, metadata.id())
                .await
                .unwrap();
            assert!(found.is_none());
        }
    }
}
