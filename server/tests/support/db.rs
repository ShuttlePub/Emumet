pub async fn truncate_tables() {
    dotenvy::dotenv().ok();

    let url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must be set for E2E tests (refusing to use default to avoid accidental data loss)");

    validate_database_url(&url);

    let pool = sqlx::PgPool::connect(&url)
        .await
        .expect("failed to connect to postgres for e2e cleanup");

    // The running Emumet server keeps its projection worker polling the
    // database roughly every 100 ms. A TRUNCATE acquires AccessExclusiveLocks
    // on many tables at once and can deadlock with that worker's transaction
    // (PostgreSQL SQLSTATE 40P01). Retry the TRUNCATE a few times when that
    // happens.
    const MAX_ATTEMPTS: usize = 5;
    for attempt in 1..=MAX_ATTEMPTS {
        let result = sqlx::query(
            "TRUNCATE accounts, account_events, auth_accounts, auth_emumet_accounts, profiles, profile_events, metadatas, metadata_events, auth_hosts, follows, remote_accounts, images, signing_keys, outbox_activities CASCADE",
        )
        .execute(&pool)
        .await;

        match result {
            Ok(_) => break,
            Err(sqlx::Error::Database(e))
                if e.code().as_deref() == Some("40P01") && attempt < MAX_ATTEMPTS =>
            {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
            Err(e) => panic!("failed to truncate e2e tables: {e}"),
        }
    }


    pool.close().await;
}

pub async fn reset_test_data() {
    truncate_tables().await;
}

pub async fn count_remote_blocks_against_local_account(account_nanoid: &str) -> i64 {
    dotenvy::dotenv().ok();

    let url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must be set for E2E tests (refusing to use default to avoid accidental data loss)");

    validate_database_url(&url);

    let pool = sqlx::PgPool::connect(&url)
        .await
        .expect("failed to connect to postgres for block assertion");

    let count: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(*) FROM blocks
        WHERE blocker_remote_id IS NOT NULL
          AND blocked_local_id = (SELECT id FROM accounts WHERE nanoid = $1)
        "#,
    )
    .bind(account_nanoid)
    .fetch_one(&pool)
    .await
    .expect("failed to count remote-to-local blocks");

    pool.close().await;
    count
}

fn validate_database_url(url: &str) {
    let parsed: url::Url = url.parse().expect("DATABASE_URL is not a valid URL");

    let host = parsed.host_str().unwrap_or("");
    let allowed_hosts = ["localhost", "127.0.0.1", "postgres", "emumet-postgres"];
    assert!(
        allowed_hosts.contains(&host),
        "E2E database host must be one of {allowed_hosts:?}, got: {host}"
    );
}
