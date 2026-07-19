use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use kernel::interfaces::repository::{DependOnOutboxActivityRepository, OutboxActivityRepository};
use kernel::prelude::entity::{AccountId, OutboxActivity};
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
        }
    }
}

pub struct PostgresOutboxActivityRepository;

impl OutboxActivityRepository for PostgresOutboxActivityRepository {
    type Executor = PostgresConnection;

    async fn create(
        &self,
        executor: &mut Self::Executor,
        activity: &OutboxActivity,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            r#"
            INSERT INTO outbox_activities (account_id, activity_id, activity_type, object_json, created_at)
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(activity.account_id.as_ref())
        .bind(&activity.activity_id)
        .bind(&activity.activity_type)
        .bind(&activity.object_json)
        .bind(activity.created_at)
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }

    async fn find_by_account_id(
        &self,
        executor: &mut Self::Executor,
        account_id: &AccountId,
        limit: usize,
        cursor: Option<i64>,
    ) -> error_stack::Result<Vec<OutboxActivity>, KernelError> {
        let con: &mut PgConnection = executor;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        sqlx::query_as::<_, OutboxActivityRow>(
            r#"
            SELECT id, account_id, activity_id, activity_type, object_json, created_at
            FROM outbox_activities
            WHERE account_id = $1 AND ($2::BIGINT IS NULL OR id < $2)
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
        executor: &mut Self::Executor,
        account_id: &AccountId,
    ) -> error_stack::Result<u64, KernelError> {
        let con: &mut PgConnection = executor;
        let count: i64 = sqlx::query_scalar(
            r#"
            SELECT COUNT(*) FROM outbox_activities WHERE account_id = $1
            "#,
        )
        .bind(account_id.as_ref())
        .fetch_one(con)
        .await
        .convert_error()?;
        Ok(count as u64)
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
        let mut executor = database.get_executor().await.unwrap();
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
        };

        database
            .outbox_activity_repository()
            .create(&mut executor, &activity)
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
        assert!(stored[0].id > 0);
    }
}
