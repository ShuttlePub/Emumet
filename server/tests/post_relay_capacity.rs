//! The shipped server entry, not AppModule: help/report must not start Ory or apply migration.
use driver::database::isolated_db::IsolatedDatabase;
use std::process::Output;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
async fn run(args: &[&str], database: &str) -> Output {
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio::process::Command::new(env!("CARGO_BIN_EXE_server"))
            .args(args)
            .env("DATABASE_URL", database)
            .env("HYDRA_ISSUER_URL", "http://127.0.0.1:9/")
            .env("KRATOS_PUBLIC_URL", "http://127.0.0.1:9/")
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("operator entry waited for normal startup")
    .unwrap()
}
fn url(db: &IsolatedDatabase) -> String {
    let mut url = url::Url::parse(&std::env::var("DATABASE_URL").unwrap()).unwrap();
    url.set_path(db.pool.connect_options().get_database().unwrap());
    url.to_string()
}
#[tokio::test]
async fn post_relay_capacity_help_needs_no_database_ory_or_master_key() {
    for args in [&["--help"][..], &["capacity", "report", "--help"][..]] {
        let out = run(args, "postgres://unreachable:invalid@127.0.0.1:9/none").await;
        assert!(out.status.success());
        let help = String::from_utf8(out.stdout).unwrap();
        assert!(help.contains("no migrations") || help.contains("No migrations"));
        assert!(help.contains("capacity report"));
    }
}
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn post_relay_capacity_command_publishes_without_migration_or_normal_startup() {
    let db = IsolatedDatabase::create().await;
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let sample = OffsetDateTime::now_utc().format(&Rfc3339).unwrap();
    let out = run(
        &[
            "capacity",
            "report",
            "--free-bytes",
            "53687091200",
            "--total-bytes",
            "107374182400",
            "--sampled-at",
            &sample,
        ],
        &url(&db),
    )
    .await;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let state: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(state["physical_accepting"], true);
    assert_eq!(state["body_count"], 0);
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(after, before);
}
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn post_relay_capacity_command_refuses_old_schema_and_does_not_apply_pending_migration() {
    let db = IsolatedDatabase::create_before_migration(20261122000300).await;
    let sample = OffsetDateTime::now_utc().format(&Rfc3339).unwrap();
    let out = run(
        &[
            "capacity",
            "report",
            "--free-bytes",
            "53687091200",
            "--total-bytes",
            "107374182400",
            "--sampled-at",
            &sample,
        ],
        &url(&db),
    )
    .await;
    assert!(!out.status.success());
    let error = String::from_utf8(out.stderr).unwrap();
    assert!(error.contains("exact schema"));
    assert!(!error.contains("postgres://"));
    let applied: i64 =
        sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations WHERE version=20261122000300")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(applied, 0);
    let table: Option<String> =
        sqlx::query_scalar("SELECT to_regclass('post_relay_capacity')::text")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(table.is_none());
}
