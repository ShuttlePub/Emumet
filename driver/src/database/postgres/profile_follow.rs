use crate::database::postgres::profile_graph::{actor_from_columns, push_actor_eq, split_actor};
use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use error_stack::Report;
use kernel::interfaces::repository::{
    BeginFollowOutcome, DependOnProfileFollowRepository, ProfileFollowRepository,
};
use kernel::prelude::entity::{
    GraphActivityId, Nanoid, ProfileFollow, ProfileFollowEndReason, ProfileFollowId,
    ProfileFollowParts, ProfileFollowState, ProfileGraphActor, ProfileId,
};
use kernel::KernelError;
use sqlx::{PgConnection, QueryBuilder};
use time::OffsetDateTime;

const COLUMNS: &str = "id, nanoid, follower_profile_id, follower_actor_url, followee_profile_id, \
                       followee_actor_url, activity_id, state, end_reason, accepted_at, ended_at, \
                       created_at, updated_at";

const BEGIN_RETRIES: usize = 3;

#[derive(sqlx::FromRow)]
struct ProfileFollowRow {
    id: i64,
    nanoid: String,
    follower_profile_id: Option<i64>,
    follower_actor_url: Option<String>,
    followee_profile_id: Option<i64>,
    followee_actor_url: Option<String>,
    activity_id: String,
    state: String,
    end_reason: Option<String>,
    accepted_at: Option<OffsetDateTime>,
    ended_at: Option<OffsetDateTime>,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

impl TryFrom<ProfileFollowRow> for ProfileFollow {
    type Error = Report<KernelError>;

    fn try_from(row: ProfileFollowRow) -> Result<Self, Self::Error> {
        ProfileFollow::from_parts(ProfileFollowParts {
            id: ProfileFollowId::new(row.id),
            nanoid: Nanoid::new(row.nanoid),
            follower: actor_from_columns(row.follower_profile_id, row.follower_actor_url)?,
            followee: actor_from_columns(row.followee_profile_id, row.followee_actor_url)?,
            activity_id: GraphActivityId::new(row.activity_id)?,
            state: ProfileFollowState::parse(&row.state)?,
            end_reason: row
                .end_reason
                .as_deref()
                .map(ProfileFollowEndReason::parse)
                .transpose()?,
            accepted_at: row.accepted_at,
            ended_at: row.ended_at,
            created_at: row.created_at,
            updated_at: row.updated_at,
        })
    }
}

fn into_follows(
    rows: Vec<ProfileFollowRow>,
) -> error_stack::Result<Vec<ProfileFollow>, KernelError> {
    rows.into_iter().map(ProfileFollow::try_from).collect()
}

fn select_builder() -> QueryBuilder<'static, sqlx::Postgres> {
    QueryBuilder::new(format!("SELECT {COLUMNS} FROM profile_follows WHERE "))
}

pub struct PostgresProfileFollowRepository;

impl ProfileFollowRepository for PostgresProfileFollowRepository {
    type Connection = PostgresConnection;

    async fn begin_generation(
        &self,
        executor: &mut Self::Connection,
        follow: &ProfileFollow,
    ) -> error_stack::Result<BeginFollowOutcome, KernelError> {
        let (follower_profile_id, follower_actor_url) = split_actor(follow.follower());
        let (followee_profile_id, followee_actor_url) = split_actor(follow.followee());
        for _ in 0..BEGIN_RETRIES {
            let con: &mut PgConnection = executor;
            // ON CONFLICT (対象指定なし) は主キー・nanoid・世代・有効な組の一意制約のどれでも拾う。
            // 競合相手が未 commit なら commit/rollback まで待つので、続く SELECT で相手の行が見える。
            let inserted = sqlx::query(
                //language=postgresql
                r#"
                INSERT INTO profile_follows (
                    id, nanoid, follower_profile_id, follower_actor_url,
                    followee_profile_id, followee_actor_url, activity_id,
                    state, end_reason, accepted_at, ended_at, created_at, updated_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
                ON CONFLICT DO NOTHING
                "#,
            )
            .bind(follow.id().as_ref())
            .bind(follow.nanoid().as_ref())
            .bind(follower_profile_id)
            .bind(follower_actor_url)
            .bind(followee_profile_id)
            .bind(followee_actor_url)
            .bind(follow.activity_id().as_str())
            .bind(follow.state().as_str())
            .bind(
                follow
                    .end_reason()
                    .as_ref()
                    .map(ProfileFollowEndReason::as_str),
            )
            .bind(follow.accepted_at())
            .bind(follow.ended_at())
            .bind(follow.created_at())
            .bind(follow.updated_at())
            .execute(con)
            .await
            .convert_error()?
            .rows_affected();
            if inserted == 1 {
                return Ok(BeginFollowOutcome::Created);
            }
            if let Some(existing) = self
                .find_by_follower_activity(executor, follow.follower(), follow.activity_id())
                .await?
            {
                return Ok(BeginFollowOutcome::ReplayedActivity(Box::new(existing)));
            }
            if let Some(existing) = self
                .find_active(executor, follow.follower(), follow.followee())
                .await?
            {
                return Ok(BeginFollowOutcome::ActiveGenerationExists(Box::new(
                    existing,
                )));
            }
        }
        Err(Report::new(KernelError::Concurrency)
            .attach_printable("profile follow generation conflicted with a concurrent change"))
    }

    async fn find_by_id(
        &self,
        executor: &mut Self::Connection,
        id: &ProfileFollowId,
    ) -> error_stack::Result<Option<ProfileFollow>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, ProfileFollowRow>(&format!(
            "SELECT {COLUMNS} FROM profile_follows WHERE id = $1"
        ))
        .bind(id.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()?
        .map(ProfileFollow::try_from)
        .transpose()
    }

    async fn find_by_nanoid(
        &self,
        executor: &mut Self::Connection,
        nanoid: &Nanoid<ProfileFollow>,
    ) -> error_stack::Result<Option<ProfileFollow>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, ProfileFollowRow>(&format!(
            "SELECT {COLUMNS} FROM profile_follows WHERE nanoid = $1"
        ))
        .bind(nanoid.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()?
        .map(ProfileFollow::try_from)
        .transpose()
    }

    async fn find_by_follower_activity(
        &self,
        executor: &mut Self::Connection,
        follower: &ProfileGraphActor,
        activity_id: &GraphActivityId,
    ) -> error_stack::Result<Option<ProfileFollow>, KernelError> {
        let con: &mut PgConnection = executor;
        let mut builder = select_builder();
        push_actor_eq(&mut builder, "follower", follower);
        builder
            .push(" AND activity_id = ")
            .push_bind(activity_id.as_str().to_owned());
        builder
            .build_query_as::<ProfileFollowRow>()
            .fetch_optional(con)
            .await
            .convert_error()?
            .map(ProfileFollow::try_from)
            .transpose()
    }

    async fn find_active(
        &self,
        executor: &mut Self::Connection,
        follower: &ProfileGraphActor,
        followee: &ProfileGraphActor,
    ) -> error_stack::Result<Option<ProfileFollow>, KernelError> {
        let con: &mut PgConnection = executor;
        let mut builder = select_builder();
        push_actor_eq(&mut builder, "follower", follower);
        builder.push(" AND ");
        push_actor_eq(&mut builder, "followee", followee);
        builder.push(" AND state <> 'ended'");
        builder
            .build_query_as::<ProfileFollowRow>()
            .fetch_optional(con)
            .await
            .convert_error()?
            .map(ProfileFollow::try_from)
            .transpose()
    }

    async fn list_generations(
        &self,
        executor: &mut Self::Connection,
        follower: &ProfileGraphActor,
        followee: &ProfileGraphActor,
    ) -> error_stack::Result<Vec<ProfileFollow>, KernelError> {
        let con: &mut PgConnection = executor;
        let mut builder = select_builder();
        push_actor_eq(&mut builder, "follower", follower);
        builder.push(" AND ");
        push_actor_eq(&mut builder, "followee", followee);
        builder.push(" ORDER BY id ASC");
        let rows = builder
            .build_query_as::<ProfileFollowRow>()
            .fetch_all(con)
            .await
            .convert_error()?;
        into_follows(rows)
    }

    async fn list_following(
        &self,
        executor: &mut Self::Connection,
        profile: &ProfileId,
        after: Option<&ProfileFollowId>,
        limit: i64,
    ) -> error_stack::Result<Vec<ProfileFollow>, KernelError> {
        list_active(executor, "follower_profile_id", profile, after, limit).await
    }

    async fn list_followers(
        &self,
        executor: &mut Self::Connection,
        profile: &ProfileId,
        after: Option<&ProfileFollowId>,
        limit: i64,
    ) -> error_stack::Result<Vec<ProfileFollow>, KernelError> {
        list_active(executor, "followee_profile_id", profile, after, limit).await
    }

    async fn accept(
        &self,
        executor: &mut Self::Connection,
        id: &ProfileFollowId,
        now: OffsetDateTime,
    ) -> error_stack::Result<bool, KernelError> {
        let con: &mut PgConnection = executor;
        let result = sqlx::query(
            //language=postgresql
            r#"
            UPDATE profile_follows
            SET state = 'accepted', accepted_at = $2, updated_at = $2
            WHERE id = $1 AND state = 'pending'
            "#,
        )
        .bind(id.as_ref())
        .bind(now)
        .execute(con)
        .await
        .convert_error()?;
        Ok(result.rows_affected() > 0)
    }

    async fn end(
        &self,
        executor: &mut Self::Connection,
        id: &ProfileFollowId,
        reason: ProfileFollowEndReason,
        now: OffsetDateTime,
    ) -> error_stack::Result<bool, KernelError> {
        let con: &mut PgConnection = executor;
        let result = sqlx::query(
            //language=postgresql
            r#"
            UPDATE profile_follows
            SET state = 'ended', end_reason = $2, ended_at = $3, updated_at = $3
            WHERE id = $1 AND state <> 'ended'
            "#,
        )
        .bind(id.as_ref())
        .bind(reason.as_str())
        .bind(now)
        .execute(con)
        .await
        .convert_error()?;
        Ok(result.rows_affected() > 0)
    }

    async fn end_active_between(
        &self,
        executor: &mut Self::Connection,
        a: &ProfileGraphActor,
        b: &ProfileGraphActor,
        reason: ProfileFollowEndReason,
        now: OffsetDateTime,
    ) -> error_stack::Result<Vec<ProfileFollow>, KernelError> {
        let con: &mut PgConnection = executor;
        let mut builder: QueryBuilder<'_, sqlx::Postgres> =
            QueryBuilder::new("UPDATE profile_follows SET state = 'ended', end_reason = ");
        builder.push_bind(reason.as_str());
        builder.push(", ended_at = ").push_bind(now);
        builder.push(", updated_at = ").push_bind(now);
        builder.push(" WHERE state <> 'ended' AND ((");
        push_actor_eq(&mut builder, "follower", a);
        builder.push(" AND ");
        push_actor_eq(&mut builder, "followee", b);
        builder.push(") OR (");
        push_actor_eq(&mut builder, "follower", b);
        builder.push(" AND ");
        push_actor_eq(&mut builder, "followee", a);
        builder.push(format!(")) RETURNING {COLUMNS}"));
        let rows = builder
            .build_query_as::<ProfileFollowRow>()
            .fetch_all(con)
            .await
            .convert_error()?;
        let mut follows = into_follows(rows)?;
        follows.sort_by_key(|follow| *follow.id().as_ref());
        Ok(follows)
    }
}

async fn list_active(
    executor: &mut PostgresConnection,
    column: &str,
    profile: &ProfileId,
    after: Option<&ProfileFollowId>,
    limit: i64,
) -> error_stack::Result<Vec<ProfileFollow>, KernelError> {
    let con: &mut PgConnection = executor;
    let mut builder = select_builder();
    builder
        .push(format!("{column} = "))
        .push_bind(*profile.as_ref());
    builder.push(" AND state <> 'ended'");
    if let Some(after) = after {
        builder.push(" AND id > ").push_bind(*after.as_ref());
    }
    builder.push(" ORDER BY id ASC LIMIT ").push_bind(limit);
    let rows = builder
        .build_query_as::<ProfileFollowRow>()
        .fetch_all(con)
        .await
        .convert_error()?;
    into_follows(rows)
}

impl DependOnProfileFollowRepository for PostgresDatabase {
    type ProfileFollowRepository = PostgresProfileFollowRepository;

    fn profile_follow_repository(&self) -> &Self::ProfileFollowRepository {
        &PostgresProfileFollowRepository
    }
}
