use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use kernel::interfaces::modify::{DependOnMetadataModifier, MetadataModifier};
use kernel::interfaces::query::{DependOnMetadataQuery, MetadataQuery};
use kernel::prelude::entity::{
    AccountId, EventVersion, Metadata, MetadataContent, MetadataId, MetadataLabel, Nanoid,
};
use kernel::KernelError;
use sqlx::PgConnection;
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct MetadataRow {
    id: Uuid,
    account_id: Uuid,
    label: String,
    content: String,
    version: Uuid,
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
            SELECT id, account_id, label, content, version, nanoid
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
        transaction: &mut Self::Transaction,
        metadata: &Metadata,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = transaction;
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
        use kernel::interfaces::modify::{
            AccountModifier, DependOnAccountModifier, DependOnMetadataModifier, MetadataModifier,
        };
        use kernel::interfaces::query::{DependOnMetadataQuery, MetadataQuery};
        use kernel::prelude::entity::{
            Account, AccountId, AccountIsBot, AccountName, AccountPrivateKey, AccountPublicKey,
            EventVersion, Metadata, MetadataContent, MetadataId, MetadataLabel, Nanoid,
        };
        use uuid::Uuid;

        #[tokio::test]
        async fn find_by_id() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let account_id = AccountId::new(Uuid::now_v7());

            let account = Account::new(
                account_id.clone(),
                AccountName::new("name".to_string()),
                AccountPrivateKey::new("private_key".to_string()),
                AccountPublicKey::new("public_key".to_string()),
                AccountIsBot::new(false),
                None,
                EventVersion::new(Uuid::now_v7()),
                Nanoid::default(),
            );

            database
                .account_modifier()
                .create(&mut transaction, &account)
                .await
                .unwrap();
            let metadata = Metadata::new(
                MetadataId::new(Uuid::now_v7()),
                account_id.clone(),
                MetadataLabel::new("label".to_string()),
                MetadataContent::new("content".to_string()),
                EventVersion::new(Uuid::now_v7()),
                Nanoid::default(),
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
                .unwrap();
            assert_eq!(found.as_ref().map(Metadata::id), Some(metadata.id()));
        }

        #[tokio::test]
        async fn find_by_account_id() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let account_id = AccountId::new(Uuid::now_v7());
            let account = Account::new(
                account_id.clone(),
                AccountName::new("name".to_string()),
                AccountPrivateKey::new("private_key".to_string()),
                AccountPublicKey::new("public_key".to_string()),
                AccountIsBot::new(false),
                None,
                EventVersion::new(Uuid::now_v7()),
                Nanoid::default(),
            );

            database
                .account_modifier()
                .create(&mut transaction, &account)
                .await
                .unwrap();
            let metadata = Metadata::new(
                MetadataId::new(Uuid::now_v7()),
                account_id.clone(),
                MetadataLabel::new("label".to_string()),
                MetadataContent::new("content".to_string()),
                EventVersion::new(Uuid::now_v7()),
                Nanoid::default(),
            );
            let metadata2 = Metadata::new(
                MetadataId::new(Uuid::now_v7()),
                account_id.clone(),
                MetadataLabel::new("label2".to_string()),
                MetadataContent::new("content2".to_string()),
                EventVersion::new(Uuid::now_v7()),
                Nanoid::default(),
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
            assert_eq!(
                found.iter().map(Metadata::id).collect::<Vec<_>>(),
                vec![metadata.id(), metadata2.id()]
            );
        }
    }
    mod modify {
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::modify::{
            AccountModifier, DependOnAccountModifier, DependOnMetadataModifier, MetadataModifier,
        };
        use kernel::interfaces::query::{DependOnMetadataQuery, MetadataQuery};
        use kernel::prelude::entity::{
            Account, AccountId, AccountIsBot, AccountName, AccountPrivateKey, AccountPublicKey,
            EventVersion, Metadata, MetadataContent, MetadataId, MetadataLabel, Nanoid,
        };
        use uuid::Uuid;

        #[tokio::test]
        async fn create() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let account_id = AccountId::new(Uuid::now_v7());
            let account = Account::new(
                account_id.clone(),
                AccountName::new("name".to_string()),
                AccountPrivateKey::new("private_key".to_string()),
                AccountPublicKey::new("public_key".to_string()),
                AccountIsBot::new(false),
                None,
                EventVersion::new(Uuid::now_v7()),
                Nanoid::default(),
            );

            database
                .account_modifier()
                .create(&mut transaction, &account)
                .await
                .unwrap();
            let metadata = Metadata::new(
                MetadataId::new(Uuid::now_v7()),
                account_id.clone(),
                MetadataLabel::new("label".to_string()),
                MetadataContent::new("content".to_string()),
                EventVersion::new(Uuid::now_v7()),
                Nanoid::default(),
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
                .unwrap();
            assert_eq!(found.as_ref().map(Metadata::id), Some(metadata.id()));
        }

        #[tokio::test]
        async fn update() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let account_id = AccountId::new(Uuid::now_v7());
            let account = Account::new(
                account_id.clone(),
                AccountName::new("name".to_string()),
                AccountPrivateKey::new("private_key".to_string()),
                AccountPublicKey::new("public_key".to_string()),
                AccountIsBot::new(false),
                None,
                EventVersion::new(Uuid::now_v7()),
                Nanoid::default(),
            );

            database
                .account_modifier()
                .create(&mut transaction, &account)
                .await
                .unwrap();
            let metadata = Metadata::new(
                MetadataId::new(Uuid::now_v7()),
                account_id.clone(),
                MetadataLabel::new("label".to_string()),
                MetadataContent::new("content".to_string()),
                EventVersion::new(Uuid::now_v7()),
                Nanoid::default(),
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
                EventVersion::new(Uuid::now_v7()),
                Nanoid::default(),
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
                .unwrap();
            assert_eq!(
                found.as_ref().map(Metadata::id),
                Some(updated_metadata.id())
            );
        }

        #[tokio::test]
        async fn delete() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let account_id = AccountId::new(Uuid::now_v7());
            let account = Account::new(
                account_id.clone(),
                AccountName::new("name".to_string()),
                AccountPrivateKey::new("private_key".to_string()),
                AccountPublicKey::new("public_key".to_string()),
                AccountIsBot::new(false),
                None,
                EventVersion::new(Uuid::now_v7()),
                Nanoid::default(),
            );

            database
                .account_modifier()
                .create(&mut transaction, &account)
                .await
                .unwrap();
            let metadata = Metadata::new(
                MetadataId::new(Uuid::now_v7()),
                account_id.clone(),
                MetadataLabel::new("label".to_string()),
                MetadataContent::new("content".to_string()),
                EventVersion::new(Uuid::now_v7()),
                Nanoid::default(),
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
