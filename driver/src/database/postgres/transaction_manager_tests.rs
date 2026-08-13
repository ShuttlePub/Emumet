use super::*;
use error_stack::Report;

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn transaction_commits_when_operation_succeeds() {
    // Given
    kernel::ensure_generator_initialized();
    let database = PostgresDatabase::new().await.unwrap();
    let id = kernel::generate_id();

    // When
    database
        .transaction(|conn| {
            Box::pin(async move {
                sqlx::query("INSERT INTO auth_hosts (id, url) VALUES ($1, $2)")
                    .bind(id)
                    .bind(format!("https://transaction-{id}.example.com"))
                    .execute(&mut **conn)
                    .await
                    .convert_error()?;
                Ok(())
            })
        })
        .await
        .unwrap();

    // Then
    let mut conn = database.connection().await.unwrap();
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM auth_hosts WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(count.0, 1);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn transaction_rolls_back_when_operation_fails() {
    // Given
    kernel::ensure_generator_initialized();
    let database = PostgresDatabase::new().await.unwrap();
    let id = kernel::generate_id();

    // When
    let result = database
        .transaction(|conn| {
            Box::pin(async move {
                sqlx::query("INSERT INTO auth_hosts (id, url) VALUES ($1, $2)")
                    .bind(id)
                    .bind(format!("https://rollback-{id}.example.com"))
                    .execute(&mut **conn)
                    .await
                    .convert_error()?;
                Err::<(), _>(Report::new(KernelError::Rejected))
            })
        })
        .await;

    // Then
    assert!(result.is_err());
    let mut conn = database.connection().await.unwrap();
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM auth_hosts WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(count.0, 0);
}
