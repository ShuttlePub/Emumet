use crate::database::postgres::profile_graph::{actor_from_columns, push_actor_eq, split_actor};
use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use error_stack::Report;
use kernel::interfaces::repository::{DependOnProfileBlockRepository, ProfileBlockRepository};
use kernel::prelude::entity::{Nanoid, ProfileBlock, ProfileBlockId, ProfileGraphActor, ProfileId};
use kernel::KernelError;
use sqlx::{PgConnection, QueryBuilder};
use time::OffsetDateTime;

const COLUMNS: &str =
    "id, nanoid, owner_profile_id, target_profile_id, target_actor_url, created_at";

#[derive(sqlx::FromRow)]
struct ProfileBlockRow {
    id: i64,
    nanoid: String,
    owner_profile_id: i64,
    target_profile_id: Option<i64>,
    target_actor_url: Option<String>,
    created_at: OffsetDateTime,
}

impl TryFrom<ProfileBlockRow> for ProfileBlock {
    type Error = Report<KernelError>;

    fn try_from(row: ProfileBlockRow) -> Result<Self, Self::Error> {
        ProfileBlock::restore(
            ProfileBlockId::new(row.id),
            Nanoid::new(row.nanoid),
            ProfileId::new(row.owner_profile_id),
            actor_from_columns(row.target_profile_id, row.target_actor_url)?,
            row.created_at,
        )
    }
}

fn select_builder() -> QueryBuilder<'static, sqlx::Postgres> {
    QueryBuilder::new(format!("SELECT {COLUMNS} FROM profile_blocks WHERE "))
}

pub struct PostgresProfileBlockRepository;

impl ProfileBlockRepository for PostgresProfileBlockRepository {
    type Connection = PostgresConnection;

    async fn insert_if_absent(
        &self,
        executor: &mut Self::Connection,
        block: &ProfileBlock,
    ) -> error_stack::Result<bool, KernelError> {
        let con: &mut PgConnection = executor;
        let (target_profile_id, target_actor_url) = split_actor(block.target());
        let result = sqlx::query(
            //language=postgresql
            r#"
            INSERT INTO profile_blocks (
                id, nanoid, owner_profile_id, target_profile_id, target_actor_url, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(block.id().as_ref())
        .bind(block.nanoid().as_ref())
        .bind(block.owner().as_ref())
        .bind(target_profile_id)
        .bind(target_actor_url)
        .bind(block.created_at())
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
    ) -> error_stack::Result<Option<ProfileBlock>, KernelError> {
        let con: &mut PgConnection = executor;
        let mut builder = select_builder();
        builder
            .push("owner_profile_id = ")
            .push_bind(*owner.as_ref());
        builder.push(" AND ");
        push_actor_eq(&mut builder, "target", target);
        builder
            .build_query_as::<ProfileBlockRow>()
            .fetch_optional(con)
            .await
            .convert_error()?
            .map(ProfileBlock::try_from)
            .transpose()
    }

    async fn is_blocked(
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
        after: Option<&ProfileBlockId>,
        limit: i64,
    ) -> error_stack::Result<Vec<ProfileBlock>, KernelError> {
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
            .build_query_as::<ProfileBlockRow>()
            .fetch_all(con)
            .await
            .convert_error()?
            .into_iter()
            .map(ProfileBlock::try_from)
            .collect()
    }

    async fn delete_by_nanoid(
        &self,
        executor: &mut Self::Connection,
        owner: &ProfileId,
        nanoid: &Nanoid<ProfileBlock>,
    ) -> error_stack::Result<bool, KernelError> {
        let con: &mut PgConnection = executor;
        let result = sqlx::query(
            //language=postgresql
            r#"
            DELETE FROM profile_blocks WHERE owner_profile_id = $1 AND nanoid = $2
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

impl DependOnProfileBlockRepository for PostgresDatabase {
    type ProfileBlockRepository = PostgresProfileBlockRepository;

    fn profile_block_repository(&self) -> &Self::ProfileBlockRepository {
        &PostgresProfileBlockRepository
    }
}
