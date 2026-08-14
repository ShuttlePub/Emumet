use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use kernel::interfaces::repository::{DependOnOutboxActivityRepository, OutboxActivityRepository};
use kernel::prelude::entity::{AccountId, OutboxActivity, OutboxActivityId};
use kernel::KernelError;
use sqlx::PgConnection;
use time::OffsetDateTime;

#[derive(sqlx::FromRow)]
struct OutboxActivityRow {
    id: i64,
    account_id: i64,
    activity_id: String,
    activity_type: String,
    object_json: String,
    created_at: OffsetDateTime,
    delivered_at: Option<OffsetDateTime>,
    attempted_at: Option<OffsetDateTime>,
    error: Option<String>,
}

impl From<OutboxActivityRow> for OutboxActivity {
    fn from(value: OutboxActivityRow) -> Self {
        OutboxActivity {
            id: value.id,
            account_id: AccountId::new(value.account_id),
            activity_id: value.activity_id,
            activity_type: value.activity_type,
            object_json: value.object_json,
            created_at: value.created_at,
            delivered_at: value.delivered_at,
            attempted_at: value.attempted_at,
            error: value.error,
        }
    }
}

pub struct PostgresOutboxActivityRepository;

impl OutboxActivityRepository for PostgresOutboxActivityRepository {
    type Connection = PostgresConnection;

    async fn create(
        &self,
        executor: &mut Self::Connection,
        activity: &OutboxActivity,
    ) -> error_stack::Result<OutboxActivityId, KernelError> {
        let con: &mut PgConnection = executor;
        let id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO outbox_activities (account_id, activity_id, activity_type, object_json, created_at)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING id
            "#,
        )
        .bind(activity.account_id.as_ref())
        .bind(&activity.activity_id)
        .bind(&activity.activity_type)
        .bind(&activity.object_json)
        .bind(activity.created_at)
        .fetch_one(con)
        .await
        .convert_error()?;
        Ok(id)
    }

    async fn find_by_account_id(
        &self,
        executor: &mut Self::Connection,
        account_id: &AccountId,
        limit: usize,
        cursor: Option<i64>,
    ) -> error_stack::Result<Vec<OutboxActivity>, KernelError> {
        let con: &mut PgConnection = executor;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        sqlx::query_as::<_, OutboxActivityRow>(
            r#"
            SELECT id, account_id, activity_id, activity_type, object_json, created_at, delivered_at, attempted_at, error
            FROM outbox_activities
            WHERE account_id = $1 AND ($2::BIGINT IS NULL OR id < $2) AND delivered_at IS NOT NULL
            ORDER BY id DESC
            LIMIT $3
            "#,
        )
        .bind(account_id.as_ref())
        .bind(cursor)
        .bind(limit)
        .fetch_all(con)
        .await
        .convert_error()
        .map(|rows| rows.into_iter().map(OutboxActivity::from).collect())
    }

    async fn count_by_account_id(
        &self,
        executor: &mut Self::Connection,
        account_id: &AccountId,
    ) -> error_stack::Result<u64, KernelError> {
        let con: &mut PgConnection = executor;
        let count: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*) FROM outbox_activities WHERE account_id = $1 AND delivered_at IS NOT NULL
            "#,
        )
        .bind(account_id.as_ref())
        .fetch_one(con)
        .await
        .convert_error()?;
        Ok(count as u64)
    }

    async fn find_pending_deliveries(
        &self,
        executor: &mut Self::Connection,
        limit: usize,
    ) -> error_stack::Result<Vec<OutboxActivity>, KernelError> {
        let con: &mut PgConnection = executor;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        sqlx::query_as::<_, OutboxActivityRow>(
            r#"
            SELECT id, account_id, activity_id, activity_type, object_json, created_at, delivered_at, attempted_at, error
            FROM outbox_activities
            WHERE delivered_at IS NULL
            ORDER BY id ASC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(con)
        .await
        .convert_error()
        .map(|rows| rows.into_iter().map(OutboxActivity::from).collect())
    }

    async fn mark_delivered(
        &self,
        executor: &mut Self::Connection,
        id: &OutboxActivityId,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            r#"
            UPDATE outbox_activities
            SET delivered_at = NOW(), attempted_at = NULL, error = NULL
            WHERE id = $1
            "#,
        )
        .bind(*id)
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }

    async fn mark_delivery_attempt(
        &self,
        executor: &mut Self::Connection,
        id: &OutboxActivityId,
        error: Option<&str>,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            r#"
            UPDATE outbox_activities
            SET attempted_at = NOW(), error = $2
            WHERE id = $1
            "#,
        )
        .bind(*id)
        .bind(error)
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }
}

impl DependOnOutboxActivityRepository for PostgresDatabase {
    type OutboxActivityRepository = PostgresOutboxActivityRepository;

    fn outbox_activity_repository(&self) -> &Self::OutboxActivityRepository {
        &PostgresOutboxActivityRepository
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::activitypub::ActorUrlBuilder;
    use kernel::interfaces::database::DatabaseConnection;
    use serde_json::json;

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn create_and_find_by_account_id_returns_stored_activity() {
        kernel::ensure_generator_initialized();
        let database = PostgresDatabase::new().await.unwrap();
        let mut executor = database.connection().await.unwrap();
        let account_id = AccountId::default();
        let activity_id = format!("https://example.com/activities/{}", kernel::generate_id());
        let activity = OutboxActivity {
            id: 0,
            account_id: account_id.clone(),
            activity_id: activity_id.clone(),
            activity_type: "Create".to_string(),
            object_json: json!({
                "@context": "https://www.w3.org/ns/activitystreams",
                "id": activity_id,
                "type": "Create",
                "actor": ActorUrlBuilder::new("https://example.com", "alice").actor_id()
            })
            .to_string(),
            created_at: OffsetDateTime::now_utc(),
            delivered_at: None,
            attempted_at: None,
            error: None,
        };

        let id = database
            .outbox_activity_repository()
            .create(&mut executor, &activity)
            .await
            .unwrap();

        database
            .outbox_activity_repository()
            .mark_delivered(&mut executor, &id)
            .await
            .unwrap();

        let stored = database
            .outbox_activity_repository()
            .find_by_account_id(&mut executor, &account_id, 10, None)
            .await
            .unwrap();
        let count = database
            .outbox_activity_repository()
            .count_by_account_id(&mut executor, &account_id)
            .await
            .unwrap();

        assert_eq!(count, 1);
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].account_id, account_id);
        assert_eq!(stored[0].activity_type, "Create");
        assert_eq!(stored[0].object_json, activity.object_json);
        assert_eq!(stored[0].id, id);
        assert!(stored[0].delivered_at.is_some());
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn pending_deliveries_excludes_delivered() {
        kernel::ensure_generator_initialized();
        let database = PostgresDatabase::new().await.unwrap();
        let mut executor = database.connection().await.unwrap();
        let account_id = AccountId::default();
        let activity_id = format!("https://example.com/activities/{}", kernel::generate_id());
        let activity = OutboxActivity {
            id: 0,
            account_id: account_id.clone(),
            activity_id: activity_id.clone(),
            activity_type: "Follow".to_string(),
            object_json: json!({"id": activity_id}).to_string(),
            created_at: OffsetDateTime::now_utc(),
            delivered_at: None,
            attempted_at: None,
            error: None,
        };

        let id = database
            .outbox_activity_repository()
            .create(&mut executor, &activity)
            .await
            .unwrap();

        let pending = database
            .outbox_activity_repository()
            .find_pending_deliveries(&mut executor, 10)
            .await
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, id);

        database
            .outbox_activity_repository()
            .mark_delivery_attempt(&mut executor, &id, Some("temporary failure"))
            .await
            .unwrap();

        let pending = database
            .outbox_activity_repository()
            .find_pending_deliveries(&mut executor, 10)
            .await
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].error.as_deref(), Some("temporary failure"));
    }
}
