use super::*;
use error_stack::Report;
use sqlx::PgConnection;

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

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn savepoint_rolls_back_failed_work_and_keeps_transaction_usable() {
    // Given
    kernel::ensure_generator_initialized();
    let database = PostgresDatabase::new().await.unwrap();
    let mut tx = database.get_transaction().await.unwrap();
    let id_a = kernel::generate_id();
    let id_b = kernel::generate_id();
    let url = format!("https://savepoint-{id_a}.example.com");

    {
        let con: &mut PgConnection = tx.connection();
        sqlx::query("INSERT INTO auth_hosts (id, url) VALUES ($1, $2)")
            .bind(id_a)
            .bind(&url)
            .execute(&mut *con)
            .await
            .unwrap();
    }

    // When: a failing statement runs inside a savepoint, then the savepoint is
    // rolled back, and the transaction remains usable.
    let savepoint = tx.savepoint().await.unwrap();
    let failed = {
        let con: &mut PgConnection = tx.connection();
        sqlx::query("INSERT INTO auth_hosts (id, url) VALUES ($1, $2)")
            .bind(id_b)
            .bind(&url)
            .execute(&mut *con)
            .await
    };
    assert!(
        failed.is_err(),
        "duplicate url must fail inside the savepoint"
    );
    {
        let con = tx.connection();
        savepoint.rollback(con).await.unwrap();
    }

    {
        let con: &mut PgConnection = tx.connection();
        sqlx::query("INSERT INTO auth_hosts (id, url) VALUES ($1, $2)")
            .bind(id_b)
            .bind(format!("https://savepoint-{id_b}.example.com"))
            .execute(&mut *con)
            .await
            .unwrap();
    }

    tx.commit().await.unwrap();

    // Then: only the first and third inserts are present.
    let mut conn = database.connection().await.unwrap();
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM auth_hosts WHERE id = ANY($1)")
        .bind(&vec![id_a, id_b])
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(count.0, 2);
}
