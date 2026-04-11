pub async fn truncate_tables() {
    dotenvy::dotenv().ok();

    let url = std::env::var("DATABASE_URL")
        .expect("DATABASE_URL must be set for E2E tests (refusing to use default to avoid accidental data loss)");

    validate_database_url(&url);

    let pool = sqlx::PgPool::connect(&url)
        .await
        .expect("failed to connect to postgres for e2e cleanup");

    sqlx::query(
        "TRUNCATE accounts, account_events, auth_accounts, auth_account_events, auth_emumet_accounts, profiles, profile_events, metadatas, metadata_events, auth_hosts, follows, remote_accounts, images, signing_keys CASCADE",
    )
    .execute(&pool)
    .await
    .expect("failed to truncate e2e tables");

    pool.close().await;
}

pub async fn reset_test_data() {
    truncate_tables().await;
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
