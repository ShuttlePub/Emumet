use crate::database::account::PgAccountInternal;
use crate::DriverError;
use kernel::interfaces::repository::ProfileRepository;
use kernel::prelude::entity::{Id, Profile};
use sqlx::{PgConnection, Pool, Postgres};

#[derive(Debug, Clone)]
pub struct ProfileDatabase {
    pool: Pool<Postgres>,
}

impl ProfileDatabase {
    pub fn new(pool: Pool<Postgres>) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl ProfileRepository for ProfileDatabase {
    async fn find_by_id(&self, id: &Id<Profile>) -> Result<Option<Profile>, DriverError> {
        let mut con = self.pool.acquire().await.map_err(DriverError::SqlX)?;
        let found = PgAccountInternal::find_by_id(id, &mut con).await?;
        Ok(found)
    }

    async fn save(&self, profile: &Profile) -> Result<(), DriverError> {
        let mut con = self.pool.acquire().await.map_err(DriverError::SqlX)?;
        PgAccountInternal::create(profile, &mut con).await?;
        Ok(())
    }

    async fn update(&self, profile: &Profile) -> Result<(), DriverError> {
        let mut con = self.pool.acquire().await.map_err(DriverError::SqlX)?;
        PgAccountInternal::update(profile, &mut con).await?;
        Ok(())
    }

    async fn delete(&self, id: &Id<Profile>) -> Result<(), DriverError> {
        let mut con = self.pool.acquire().await.map_err(DriverError::SqlX)?;
        PgAccountInternal::delete(id, &mut con).await?;
        Ok(())
    }
}

pub(in crate::database) struct ProfileRow {
    id: i64,
    display_name: String,
    summary: String,
    icon: String,
    banner: String,
}

pub(in crate::database) struct ProfileInternal;

impl PgAccountInternal {
    pub async fn create(profile: &Profile, con: &mut PgConnection) -> Result<(), DriverError> {
        // language=sql
        sqlx::query(
            r#"INSERT INTO profiles (account_id, display_name, summary, icon, banner) VALUES ($1, $2, $3, $4, $5)"#
        ).bind(profile.account_id().as_ref())
            .bind(profile.display_name().as_ref())
            .bind(profile.summary().as_ref())
            .bind(profile.icon().as_ref())
            .bind(profile.banner().as_ref())
            .execute(con)
            .await?;
        Ok(())
    }

    pub async fn update(profile: &Profile, con: &mut PgConnection) -> Result<(), DriverError> {
        // language=sql
        sqlx::query(
            r#"UPDATE profiles SET display_name = $1, summary = $2, icon = $3, banner = $4 WHERE account_id = $5"#
        ).bind(profile.display_name().as_ref())
            .bind(profile.summary().as_ref())
            .bind(profile.icon().as_ref())
            .bind(profile.banner().as_ref())
            .bind(profile.account_id().as_ref())
            .execute(con)
            .await?;
        Ok(())
    }

    pub async fn delete(id: &Id<Profile>, con: &mut PgConnection) -> Result<(), DriverError> {
        // language=sql
        sqlx::query(r#"DELETE FROM profiles WHERE id = $1"#)
            .bind(id.as_ref())
            .execute(con)
            .await?;
        Ok(())
    }

    pub async fn find_by_id(
        id: &Id<Profile>,
        con: &mut PgConnection,
    ) -> Result<Option<Profile>, DriverError> {
        // language=sql
        let row: Option<ProfileRow> = sqlx::query_as(r#"SELECT * FROM profiles WHERE id = $1"#)
            .bind(id.as_ref())
            .fetch_optional(con)
            .await?;
        let row = match row {
            Some(row) => row,
            None => return Ok(None),
        };
        let profile = Profile::new(
            Id::new(row.id),
            row.display_name,
            row.summary,
            row.icon,
            row.banner,
        );
        Ok(Some(profile))
    }
}
