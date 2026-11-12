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
            "TRUNCATE accounts, account_events, auth_accounts, auth_emumet_accounts, profiles, profile_events, metadatas, metadata_events, projection_checkpoints, auth_hosts, follows, remote_accounts, images, signing_keys, outbox_activities, profile_signing_keys, profile_follows, profile_blocks, profile_mutes, profile_inbound_activities, profile_delivery_jobs, profile_delivery_attempts CASCADE",
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

async fn connect(purpose: &str) -> sqlx::PgPool {
    dotenvy::dotenv().ok();

    let url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must be set for E2E tests (refusing to use default to avoid accidental data loss)");

    validate_database_url(&url);

    sqlx::PgPool::connect(&url)
        .await
        .unwrap_or_else(|e| panic!("failed to connect to postgres for {purpose}: {e}"))
}

/// The nanoid of the (only) Profile of a personal Account: the path component of its actor URL.
pub async fn profile_nanoid_of_account(account_nanoid: &str) -> String {
    let pool = connect("profile lookup").await;
    let nanoid: String = sqlx::query_scalar(
        "SELECT p.nanoid FROM profiles p JOIN accounts a ON a.id = p.account_id WHERE a.nanoid = $1",
    )
    .bind(account_nanoid)
    .fetch_one(&pool)
    .await
    .expect("the Account must have a Profile");
    pool.close().await;
    nanoid
}

/// State of the latest delivery job of the Profile for the given activity type:
/// `(state, attempt_count)`. `None` when there is no such job.
pub async fn delivery_job_state(
    profile_nanoid: &str,
    activity_type: &str,
) -> Option<(String, i32)> {
    let pool = connect("delivery job assertion").await;
    let row: Option<(String, i32)> = sqlx::query_as(
        r#"
        SELECT state, attempt_count
        FROM profile_delivery_jobs
        WHERE sender_profile_id = (SELECT id FROM profiles WHERE nanoid = $1)
          AND activity_type = $2
        ORDER BY id DESC
        LIMIT 1
        "#,
    )
    .bind(profile_nanoid)
    .bind(activity_type)
    .fetch_optional(&pool)
    .await
    .expect("failed to query delivery job state");
    pool.close().await;
    row
}

/// Number of delivery jobs of the Profile (any state) for the given activity type.
pub async fn delivery_job_count(profile_nanoid: &str, activity_type: &str) -> i64 {
    let pool = connect("delivery job count").await;
    let count: i64 = sqlx::query_scalar(
        r#"
        SELECT count(*) FROM profile_delivery_jobs
        WHERE sender_profile_id = (SELECT id FROM profiles WHERE nanoid = $1)
          AND activity_type = $2
        "#,
    )
    .bind(profile_nanoid)
    .bind(activity_type)
    .fetch_one(&pool)
    .await
    .expect("failed to count delivery jobs");
    pool.close().await;
    count
}

/// The active key of the Profile: `(key_id_uri, encrypted_private_key JSON, public_key_pem)`.
pub async fn active_profile_key(profile_nanoid: &str) -> (String, serde_json::Value, String) {
    let pool = connect("profile key lookup").await;
    let row: (String, serde_json::Value, String) = sqlx::query_as(
        r#"
        SELECT k.key_id_uri, k.encrypted_private_key, k.public_key_pem
        FROM profile_signing_keys k JOIN profiles p ON p.id = k.profile_id
        WHERE p.nanoid = $1 AND k.revoked_at IS NULL
        "#,
    )
    .bind(profile_nanoid)
    .fetch_one(&pool)
    .await
    .expect("the Profile must have an active key");
    pool.close().await;
    row
}

/// Rows of the legacy Account federation tables (the cutover emptied them and nothing writes
/// them any more).
pub async fn legacy_federation_row_count() -> i64 {
    let pool = connect("legacy table assertion").await;
    let count: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM follows) + (SELECT count(*) FROM blocks) \
         + (SELECT count(*) FROM mutes) + (SELECT count(*) FROM remote_accounts) \
         + (SELECT count(*) FROM outbox_activities) + (SELECT count(*) FROM signing_keys)",
    )
    .fetch_one(&pool)
    .await
    .expect("failed to count legacy rows");
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
