use crate::DriverError;
use kernel::interfaces::repository::MetadataRepository;
use kernel::prelude::entity::{Account, Id, Metadata};
use kernel::KernelError;
use sqlx::{PgConnection, Pool, Postgres};

#[derive(Debug, Clone)]
pub struct MetadataDatabase {
    pool: Pool<Postgres>,
}

impl MetadataDatabase {
    pub fn new(pool: Pool<Postgres>) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl MetadataRepository for MetadataDatabase {
    async fn find_by_id(&self, id: &Id<Metadata>) -> Result<Option<Metadata>, KernelError> {
        let mut con = self.pool.acquire().await.map_err(DriverError::SqlX)?;
        let found = MetadataInternal::find_by_id(id, &mut con).await?;
        Ok(found)
    }

    async fn find_by_account_id(
        &self,
        account_id: &Id<Account>,
    ) -> Result<Vec<Metadata>, KernelError> {
        let mut con = self.pool.acquire().await.map_err(DriverError::SqlX)?;
        let found = MetadataInternal::find_by_account_id(account_id, &mut con).await?;
        Ok(found)
    }

    async fn save(&self, metadata: &Metadata) -> Result<(), KernelError> {
        let mut con = self.pool.acquire().await.map_err(DriverError::SqlX)?;
        MetadataInternal::create(metadata, &mut con).await?;
        Ok(())
    }

    async fn update(&self, metadata: &Metadata) -> Result<(), KernelError> {
        let mut con = self.pool.acquire().await.map_err(DriverError::SqlX)?;
        MetadataInternal::update(metadata, &mut con).await?;
        Ok(())
    }

    async fn delete(&self, account_id: &Id<Account>) -> Result<(), KernelError> {
        let mut con = self.pool.acquire().await.map_err(DriverError::SqlX)?;
        MetadataInternal::delete(account_id, &mut con).await?;
        Ok(())
    }
}

pub(in crate::database) struct MetadataRow {
    id: i64,
    account_id: i64,
    key: String,
    value: String,
}

pub(in crate::database) struct MetadataInternal;

impl MetadataInternal {
    pub async fn create(metadata: &Metadata, con: &mut PgConnection) -> Result<(), DriverError> {
        // language=sql
        sqlx::query(
            r#"INSERT INTO metadatas (id, account_id, label, content) VALUES ($1, $2, $3, $4)"#,
        )
        .bind(metadata.id().as_ref())
        .bind(metadata.account_id().as_ref())
        .bind(metadata.key().as_ref())
        .bind(metadata.value().as_ref())
        .execute(con)
        .await?;
        Ok(())
    }

    pub async fn update(metadata: &Metadata, con: &mut PgConnection) -> Result<(), DriverError> {
        // language=sql
        sqlx::query(r#"UPDATE metadatas SET label = $1, content = $2 WHERE id = $3"#)
            .bind(metadata.key().as_ref())
            .bind(metadata.value().as_ref())
            .bind(metadata.id().as_ref())
            .execute(con)
            .await?;
        Ok(())
    }

    pub async fn delete(
        account_id: &Id<Account>,
        con: &mut PgConnection,
    ) -> Result<(), DriverError> {
        // language=sql
        sqlx::query(r#"DELETE FROM metadatas WHERE account_id = $1"#)
            .bind(account_id.as_ref())
            .execute(con)
            .await?;
        Ok(())
    }

    pub async fn find_by_id(
        id: &Id<Metadata>,
        con: &mut PgConnection,
    ) -> Result<Option<Metadata>, DriverError> {
        // language=sql
        let row = sqlx::query_as::<_, MetadataRow>(r#"SELECT * FROM metadatas WHERE id = $1"#)
            .bind(id.as_ref())
            .fetch_optional(con)
            .await?;
        let metadata = row
            .map(|row| Metadata::new(Id::new(row.id), Id::new(row.account_id), row.key, row.value));
        Ok(metadata)
    }

    pub async fn find_by_account_id(
        account_id: &Id<Account>,
        con: &mut PgConnection,
    ) -> Result<Vec<Metadata>, DriverError> {
        // language=sql
        let rows =
            sqlx::query_as::<_, MetadataRow>(r#"SELECT * FROM metadatas WHERE account_id = $1"#)
                .bind(account_id.as_ref())
                .fetch_all(con)
                .await?;
        let metadatas = rows
            .into_iter()
            .map(|row| Metadata::new(Id::new(row.id), Id::new(row.account_id), row.key, row.value))
            .collect();
        Ok(metadatas)
    }
}
