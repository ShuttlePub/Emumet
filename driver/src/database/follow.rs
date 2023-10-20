use crate::DriverError;
use kernel::interfaces::repository::FollowRepository;
use kernel::prelude::entity::{AccountId, Follow, FollowAccountId, FollowId, RemoteAccountId};
use kernel::KernelError;
use sqlx::{PgConnection, Pool, Postgres};

#[derive(Debug, Clone)]
pub struct FollowDatabase {
    pool: Pool<Postgres>,
}

impl FollowDatabase {
    pub fn new(pool: Pool<Postgres>) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl FollowRepository for FollowDatabase {
    async fn find_by_id(&self, id: &FollowId) -> Result<Option<Follow>, KernelError> {
        let mut con = self.pool.acquire().await.map_err(DriverError::SqlX)?;
        let found = PgFollowInternal::find_by_id(id, &mut con).await?;
        Ok(found)
    }

    async fn find_by_source_id(&self, id: &FollowAccountId) -> Result<Vec<Follow>, KernelError> {
        let mut con = self.pool.acquire().await.map_err(DriverError::SqlX)?;
        let found = PgFollowInternal::find_by_source_id(id, &mut con).await?;
        Ok(found)
    }

    async fn find_by_target_id(&self, id: &FollowAccountId) -> Result<Vec<Follow>, KernelError> {
        let mut con = self.pool.acquire().await.map_err(DriverError::SqlX)?;
        let found = PgFollowInternal::find_by_target_id(id, &mut con).await?;
        Ok(found)
    }

    async fn save(&self, follow: &Follow) -> Result<(), KernelError> {
        let mut con = self.pool.acquire().await.map_err(DriverError::SqlX)?;
        PgFollowInternal::create(follow, &mut con).await?;
        Ok(())
    }

    async fn update(&self, follow: &Follow) -> Result<(), KernelError> {
        let mut con = self.pool.acquire().await.map_err(DriverError::SqlX)?;
        PgFollowInternal::update(follow, &mut con).await?;
        Ok(())
    }

    async fn delete(&self, id: &FollowId) -> Result<(), KernelError> {
        let mut con = self.pool.acquire().await.map_err(DriverError::SqlX)?;
        PgFollowInternal::delete(id, &mut con).await?;
        Ok(())
    }
}

#[derive(sqlx::FromRow)]
pub(in crate::database) struct FollowRow {
    id: i64,
    source_local: Option<i64>,
    source_remote: Option<i64>,
    destination_local: Option<i64>,
    destination_remote: Option<i64>,
}

impl TryFrom<FollowRow> for Follow {
    type Error = DriverError;
    fn try_from(row: FollowRow) -> Result<Self, Self::Error> {
        let source = FollowAccountId::new(
            row.source_local.map(AccountId::new),
            row.source_remote.map(RemoteAccountId::new),
        )?;
        let destination = FollowAccountId::new(
            row.destination_local.map(AccountId::new),
            row.destination_remote.map(RemoteAccountId::new),
        )?;
        Follow::new(FollowId::new(row.id), source, destination).map_err(DriverError::Kernel)
    }
}

pub(in crate::database) struct PgFollowInternal;

impl PgFollowInternal {
    pub async fn find_by_id(
        id: &FollowId,
        con: &mut PgConnection,
    ) -> Result<Option<Follow>, DriverError> {
        //language=sql
        sqlx::query_as::<_, FollowRow>(
            r#"SELECT id, source_local, source_remote, destination_local, destination_remote FROM follows WHERE id = $1"#,
        ).bind(id.as_ref())
            .fetch_optional(con)
            .await?
            .map(TryInto::try_into)
            .transpose()
    }

    pub async fn find_by_source_id(
        id: &FollowAccountId,
        con: &mut PgConnection,
    ) -> Result<Vec<Follow>, DriverError> {
        let (local, remote) = match id {
            FollowAccountId::Local(local) => (Some(local), None),
            FollowAccountId::Remote(remote) => (None, Some(remote)),
        };
        //language=sql
        sqlx::query_as::<_, FollowRow>(
            r#"SELECT id, source_local, source_remote, destination_local, destination_remote FROM follows WHERE source_local = $1 OR source_remote = $2"#,
        ).bind(local.map(AsRef::as_ref))
            .bind(remote.map(AsRef::as_ref))
            .fetch_all(con)
            .await?
            .into_iter()
            .map(TryInto::try_into)
            .collect()
    }

    pub async fn find_by_target_id(
        id: &FollowAccountId,
        con: &mut PgConnection,
    ) -> Result<Vec<Follow>, DriverError> {
        let (local, remote) = match id {
            FollowAccountId::Local(local) => (Some(local), None),
            FollowAccountId::Remote(remote) => (None, Some(remote)),
        };
        // language=sql
        sqlx::query_as::<_, FollowRow>(
            r#"SELECT id, source_local, source_remote, destination_local, destination_remote FROM follows WHERE destination_local = $1 OR destination_remote= $2"#
        ).bind(local.map(AsRef::as_ref))
            .bind(remote.map(AsRef::as_ref))
            .fetch_all(con)
            .await?
            .into_iter()
            .map(TryInto::try_into)
            .collect()
    }

    pub async fn create(follow: &Follow, con: &mut PgConnection) -> Result<(), DriverError> {
        let (source_local, source_remote) = match follow.source() {
            FollowAccountId::Local(local) => (Some(local), None),
            FollowAccountId::Remote(remote) => (None, Some(remote)),
        };
        let (destination_local, destination_remote) = match follow.destination() {
            FollowAccountId::Local(local) => (Some(local), None),
            FollowAccountId::Remote(remote) => (None, Some(remote)),
        };
        //language=sql
        sqlx::query(
            r#"INSERT INTO follows (id, source_local, source_remote, destination_local, destination_remote) VALUES ($1, $2, $3, $4, $5)"#,
        ).bind(follow.id().as_ref())
            .bind(source_local.map(AsRef::as_ref))
            .bind(source_remote.map(AsRef::as_ref))
            .bind(destination_local.map(AsRef::as_ref))
            .bind(destination_remote.map(AsRef::as_ref))
            .execute(con)
            .await?;

        Ok(())
    }

    pub async fn update(follow: &Follow, con: &mut PgConnection) -> Result<(), DriverError> {
        let (source_local, source_remote) = match follow.source() {
            FollowAccountId::Local(local) => (Some(local), None),
            FollowAccountId::Remote(remote) => (None, Some(remote)),
        };
        let (destination_local, destination_remote) = match follow.destination() {
            FollowAccountId::Local(local) => (Some(local), None),
            FollowAccountId::Remote(remote) => (None, Some(remote)),
        };
        //language=sql
        sqlx::query(
            r#"UPDATE follows SET source_local = $1, source_remote = $2, destination_local = $3, destination_remote = $4 WHERE id = $5"#,
        ).bind(source_local.map(AsRef::as_ref))
            .bind(source_remote.map(AsRef::as_ref))
            .bind(destination_local.map(AsRef::as_ref))
            .bind(destination_remote.map(AsRef::as_ref))
            .bind(follow.id().as_ref())
            .execute(con)
            .await?;

        Ok(())
    }

    pub async fn delete(id: &FollowId, con: &mut PgConnection) -> Result<(), DriverError> {
        //language=sql
        sqlx::query(r#"DELETE FROM follows WHERE id = $1"#)
            .bind(id.as_ref())
            .execute(con)
            .await?;

        Ok(())
    }
}
