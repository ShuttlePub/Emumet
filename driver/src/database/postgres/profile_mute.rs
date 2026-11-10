use crate::database::postgres::profile_graph::{actor_from_columns, push_actor_eq, split_actor};
use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use error_stack::Report;
use kernel::interfaces::repository::{DependOnProfileMuteRepository, ProfileMuteRepository};
use kernel::prelude::entity::{Nanoid, ProfileGraphActor, ProfileId, ProfileMute, ProfileMuteId};
use kernel::KernelError;
use sqlx::{PgConnection, QueryBuilder};
use time::OffsetDateTime;

const COLUMNS: &str =
    "id, nanoid, owner_profile_id, target_profile_id, target_actor_url, created_at";

#[derive(sqlx::FromRow)]
struct ProfileMuteRow {
    id: i64,
    nanoid: String,
    owner_profile_id: i64,
    target_profile_id: Option<i64>,
    target_actor_url: Option<String>,
    created_at: OffsetDateTime,
}

impl TryFrom<ProfileMuteRow> for ProfileMute {
    type Error = Report<KernelError>;

    fn try_from(row: ProfileMuteRow) -> Result<Self, Self::Error> {
        ProfileMute::restore(
            ProfileMuteId::new(row.id),
            Nanoid::new(row.nanoid),
            ProfileId::new(row.owner_profile_id),
            actor_from_columns(row.target_profile_id, row.target_actor_url)?,
            row.created_at,
        )
    }
}

fn select_builder() -> QueryBuilder<'static, sqlx::Postgres> {
    QueryBuilder::new(format!("SELECT {COLUMNS} FROM profile_mutes WHERE "))
}

pub struct PostgresProfileMuteRepository;

impl ProfileMuteRepository for PostgresProfileMuteRepository {
    type Connection = PostgresConnection;

    async fn insert_if_absent(
        &self,
        executor: &mut Self::Connection,
        mute: &ProfileMute,
    ) -> error_stack::Result<bool, KernelError> {
        let con: &mut PgConnection = executor;
        let (target_profile_id, target_actor_url) = split_actor(mute.target());
        let result = sqlx::query(
            //language=postgresql
            r#"
            INSERT INTO profile_mutes (
                id, nanoid, owner_profile_id, target_profile_id, target_actor_url, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(mute.id().as_ref())
        .bind(mute.nanoid().as_ref())
        .bind(mute.owner().as_ref())
        .bind(target_profile_id)
        .bind(target_actor_url)
        .bind(mute.created_at())
        .execute(con)
        .await
        .convert_error()?;
        Ok(result.rows_affected() > 0)
    }

    async fn find(
        &self,
        executor: &mut Self::Connection,
        owner: &ProfileId,
        target: &ProfileGraphActor,
    ) -> error_stack::Result<Option<ProfileMute>, KernelError> {
        let con: &mut PgConnection = executor;
        let mut builder = select_builder();
        builder
            .push("owner_profile_id = ")
            .push_bind(*owner.as_ref());
        builder.push(" AND ");
        push_actor_eq(&mut builder, "target", target);
        builder
            .build_query_as::<ProfileMuteRow>()
            .fetch_optional(con)
            .await
            .convert_error()?
            .map(ProfileMute::try_from)
            .transpose()
    }

    async fn is_muted(
        &self,
        executor: &mut Self::Connection,
        owner: &ProfileId,
        target: &ProfileGraphActor,
    ) -> error_stack::Result<bool, KernelError> {
        Ok(self.find(executor, owner, target).await?.is_some())
    }

    async fn list(
        &self,
        executor: &mut Self::Connection,
        owner: &ProfileId,
        after: Option<&ProfileMuteId>,
        limit: i64,
    ) -> error_stack::Result<Vec<ProfileMute>, KernelError> {
        let con: &mut PgConnection = executor;
        let mut builder = select_builder();
        builder
            .push("owner_profile_id = ")
            .push_bind(*owner.as_ref());
        if let Some(after) = after {
            builder.push(" AND id > ").push_bind(*after.as_ref());
        }
        builder.push(" ORDER BY id ASC LIMIT ").push_bind(limit);
        builder
            .build_query_as::<ProfileMuteRow>()
            .fetch_all(con)
            .await
            .convert_error()?
            .into_iter()
            .map(ProfileMute::try_from)
            .collect()
    }

    async fn delete_by_nanoid(
        &self,
        executor: &mut Self::Connection,
        owner: &ProfileId,
        nanoid: &Nanoid<ProfileMute>,
    ) -> error_stack::Result<bool, KernelError> {
        let con: &mut PgConnection = executor;
        let result = sqlx::query(
            //language=postgresql
            r#"
            DELETE FROM profile_mutes WHERE owner_profile_id = $1 AND nanoid = $2
            "#,
        )
        .bind(owner.as_ref())
        .bind(nanoid.as_ref())
        .execute(con)
        .await
        .convert_error()?;
        Ok(result.rows_affected() > 0)
    }
}

impl DependOnProfileMuteRepository for PostgresDatabase {
    type ProfileMuteRepository = PostgresProfileMuteRepository;

    fn profile_mute_repository(&self) -> &Self::ProfileMuteRepository {
        &PostgresProfileMuteRepository
    }
}
