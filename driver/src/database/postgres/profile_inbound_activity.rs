use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use kernel::interfaces::repository::{
    DependOnInboundActivityRepository, InboundActivityRepository,
};
use kernel::prelude::entity::{
    GraphActivityId, GraphActorUrl, InboundActivityReceipt, InboundActivityRecord,
    NewInboundActivity, ProfileId,
};
use kernel::KernelError;
use sqlx::PgConnection;
use time::OffsetDateTime;

#[derive(sqlx::FromRow)]
struct InboundActivityRow {
    receiver_profile_id: i64,
    actor_url: String,
    activity_id: String,
    activity_type: String,
    receive_count: i32,
    first_received_at: OffsetDateTime,
    last_received_at: OffsetDateTime,
}

pub struct PostgresInboundActivityRepository;

impl InboundActivityRepository for PostgresInboundActivityRepository {
    type Connection = PostgresConnection;

    async fn record(
        &self,
        executor: &mut Self::Connection,
        activity: &NewInboundActivity,
    ) -> error_stack::Result<InboundActivityReceipt, KernelError> {
        let con: &mut PgConnection = executor;
        let inserted = sqlx::query(
            //language=postgresql
            r#"
            INSERT INTO profile_inbound_activities (
                receiver_profile_id, actor_url, activity_id, activity_type,
                receive_count, first_received_at, last_received_at
            )
            VALUES ($1, $2, $3, $4, 1, $5, $5)
            ON CONFLICT (receiver_profile_id, actor_url, activity_id) DO NOTHING
            "#,
        )
        .bind(activity.receiver.as_ref())
        .bind(activity.actor.as_str())
        .bind(activity.activity_id.as_str())
        .bind(&activity.activity_type)
        .bind(activity.received_at)
        .execute(&mut *con)
        .await
        .convert_error()?
        .rows_affected()
            == 1;
        if inserted {
            return Ok(InboundActivityReceipt {
                first_seen: true,
                receive_count: 1,
            });
        }
        // 競合した相手が未 commit なら commit まで待つので、ここでは既存の行を更新できる。
        let receive_count: i32 = sqlx::query_scalar(
            //language=postgresql
            r#"
            UPDATE profile_inbound_activities SET
                receive_count = receive_count + 1,
                last_received_at = GREATEST(last_received_at, $4)
            WHERE receiver_profile_id = $1 AND actor_url = $2 AND activity_id = $3
            RETURNING receive_count
            "#,
        )
        .bind(activity.receiver.as_ref())
        .bind(activity.actor.as_str())
        .bind(activity.activity_id.as_str())
        .bind(activity.received_at)
        .fetch_one(con)
        .await
        .convert_error()?;
        let first_seen = false;
        Ok(InboundActivityReceipt {
            first_seen,
            receive_count,
        })
    }

    async fn find(
        &self,
        executor: &mut Self::Connection,
        receiver: &ProfileId,
        actor: &GraphActorUrl,
        activity_id: &GraphActivityId,
    ) -> error_stack::Result<Option<InboundActivityRecord>, KernelError> {
        let con: &mut PgConnection = executor;
        let row = sqlx::query_as::<_, InboundActivityRow>(
            //language=postgresql
            r#"
            SELECT receiver_profile_id, actor_url, activity_id, activity_type,
                   receive_count, first_received_at, last_received_at
            FROM profile_inbound_activities
            WHERE receiver_profile_id = $1 AND actor_url = $2 AND activity_id = $3
            "#,
        )
        .bind(receiver.as_ref())
        .bind(actor.as_str())
        .bind(activity_id.as_str())
        .fetch_optional(con)
        .await
        .convert_error()?;
        row.map(|row| {
            Ok(InboundActivityRecord {
                receiver: ProfileId::new(row.receiver_profile_id),
                actor: GraphActorUrl::new(row.actor_url)?,
                activity_id: GraphActivityId::new(row.activity_id)?,
                activity_type: row.activity_type,
                receive_count: row.receive_count,
                first_received_at: row.first_received_at,
                last_received_at: row.last_received_at,
            })
        })
        .transpose()
    }

    async fn purge_received_before(
        &self,
        executor: &mut Self::Connection,
        cutoff: OffsetDateTime,
    ) -> error_stack::Result<u64, KernelError> {
        let con: &mut PgConnection = executor;
        let result = sqlx::query(
            //language=postgresql
            r#"
            DELETE FROM profile_inbound_activities WHERE first_received_at < $1
            "#,
        )
        .bind(cutoff)
        .execute(con)
        .await
        .convert_error()?;
        Ok(result.rows_affected())
    }
}

impl DependOnInboundActivityRepository for PostgresDatabase {
    type InboundActivityRepository = PostgresInboundActivityRepository;

    fn inbound_activity_repository(&self) -> &Self::InboundActivityRepository {
        &PostgresInboundActivityRepository
    }
}
