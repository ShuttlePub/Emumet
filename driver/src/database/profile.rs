use crate::DriverError;
use kernel::interfaces::repository::ProfileRepository;
use kernel::prelude::entity::{
    AccountId, Profile, ProfileBanner, ProfileDisplayName, ProfileIcon, ProfileSummary,
};
use kernel::KernelError;
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
    async fn find_by_id(&self, id: &AccountId) -> Result<Option<Profile>, KernelError> {
        let mut con = self.pool.acquire().await.map_err(DriverError::SqlX)?;
        let found = PgProfileInternal::find_by_id(id, &mut con).await?;
        Ok(found)
    }

    async fn save(&self, profile: &Profile) -> Result<(), KernelError> {
        let mut con = self.pool.acquire().await.map_err(DriverError::SqlX)?;
        PgProfileInternal::create(profile, &mut con).await?;
        Ok(())
    }

    async fn update(&self, profile: &Profile) -> Result<(), KernelError> {
        let mut con = self.pool.acquire().await.map_err(DriverError::SqlX)?;
        PgProfileInternal::update(profile, &mut con).await?;
        Ok(())
    }

    async fn delete(&self, profile: &Profile) -> Result<(), KernelError> {
        let mut con = self.pool.acquire().await.map_err(DriverError::SqlX)?;
        PgProfileInternal::delete(profile, &mut con).await?;
        Ok(())
    }
}

#[derive(sqlx::FromRow)]
pub(in crate::database) struct ProfileRow {
    id: i64,
    display_name: String,
    summary: String,
    icon: String,
    banner: String,
}

fn to_profile(row: ProfileRow) -> Profile {
    Profile::new(
        AccountId::new(row.id),
        ProfileDisplayName::new(row.display_name),
        ProfileSummary::new(row.summary),
        ProfileIcon::new(row.icon),
        ProfileBanner::new(row.banner),
    )
}

fn to_profile_with_result(row: ProfileRow) -> Result<Profile, DriverError> {
    Ok(to_profile(row))
}

pub(in crate::database) struct PgProfileInternal;

impl PgProfileInternal {
    pub async fn create(profile: &Profile, con: &mut PgConnection) -> Result<(), DriverError> {
        // language=sql
        sqlx::query(
            r#"INSERT INTO profiles (account_id, display_name, summary, icon, banner) VALUES ($1, $2, $3, $4, $5)"#
        ).bind(profile.id().as_ref())
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
            .bind(profile.id().as_ref())
            .execute(con)
            .await?;
        Ok(())
    }

    pub async fn delete(profile: &Profile, con: &mut PgConnection) -> Result<(), DriverError> {
        // language=sql
        sqlx::query(r#"DELETE FROM profiles WHERE id = $1"#)
            .bind(profile.id().as_ref())
            .execute(con)
            .await?;
        Ok(())
    }

    pub async fn find_by_id(
        id: &AccountId,
        con: &mut PgConnection,
    ) -> Result<Option<Profile>, DriverError> {
        // language=sql
        sqlx::query_as(r#"SELECT * FROM profiles WHERE id = $1"#)
            .bind(id.as_ref())
            .fetch_optional(con)
            .await?
            .map(to_profile_with_result)
            .transpose()
    }
}
