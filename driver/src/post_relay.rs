//! Operator-only, connect-without-migration entry. No AppModule, listener, Ory or signing key.
use crate::database::PostgresDatabase;
use crate::ConvertError;
use error_stack::Report;
use kernel::interfaces::database::{Transaction, TransactionalDatabaseConnection};
use kernel::interfaces::post_relay::{
    CapacityReport, DependOnPostRelayStore, PostRelayStore, RelayCapacity,
};
use kernel::KernelError;
use sqlx::{PgPool, Row};

/// Exact migration set AND checksums, including no dirty/pending/unknown versions. Never run it.
pub async fn verify_schema(pool: &PgPool) -> error_stack::Result<(), KernelError> {
    let rows =
        sqlx::query("SELECT version,success,checksum FROM _sqlx_migrations ORDER BY version")
            .fetch_all(pool)
            .await
            .convert_error()?;
    let expected = sqlx::migrate!("../migrations");
    if rows.len() != expected.migrations.len()
        || rows.iter().zip(expected.iter()).any(|(r, m)| {
            r.get::<i64, _>("version") != m.version
                || !r.get::<bool, _>("success")
                || r.get::<Vec<u8>, _>("checksum").as_slice() != m.checksum.as_ref()
        })
    {
        return Err(Report::new(KernelError::Rejected).attach_printable("post relay schema mismatch; deploy matching server, do not migrate with an operator command"));
    }
    Ok(())
}
/// Operator connection: no migrations and a bounded pool; schema MUST match this binary.
pub async fn connect_operator(url: &str) -> error_stack::Result<PostgresDatabase, KernelError> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .acquire_timeout(std::time::Duration::from_secs(3))
        .connect(url)
        .await
        .convert_error()?;
    verify_schema(&pool).await?;
    Ok(PostgresDatabase::from_pool(pool))
}
/// Body-free operator result, read without lifecycle locks.
pub async fn operation_status(
    db: &PostgresDatabase,
    id: &str,
) -> error_stack::Result<serde_json::Value, KernelError> {
    use kernel::interfaces::database::DatabaseConnection;
    let mut c = db.connection().await?;
    let row=sqlx::query("SELECT selected_count,selected_bytes,staged,succeeded,failed,discarded,skipped,cancelled,cursor>=boundary AS complete,(SELECT count(*) FROM post_relay_jobs j WHERE j.operation_id=o.id) AS pending FROM post_relay_operations o WHERE id=$1").bind(id).fetch_one(&mut *c).await.convert_error()?;
    let mut v = serde_json::Map::new();
    for name in [
        "selected_count",
        "selected_bytes",
        "staged",
        "succeeded",
        "failed",
        "discarded",
        "skipped",
        "cancelled",
        "pending",
    ] {
        v.insert(name.into(), row.get::<i64, _>(name).into());
    }
    v.insert("complete".into(), row.get::<bool, _>("complete").into());
    Ok(v.into())
}
pub async fn publish_capacity(
    url: &str,
    report: &CapacityReport,
) -> error_stack::Result<RelayCapacity, KernelError> {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(3))
        .connect(url)
        .await
        .convert_error()?;
    verify_schema(&pool).await?;
    let db = PostgresDatabase::from_pool(pool.clone());
    let mut transaction = db.get_transaction().await?;
    // SET LOCAL only on this operator transaction; never on the server's shared pool.
    sqlx::query(
        "SELECT set_config('statement_timeout','3s',true),set_config('lock_timeout','2s',true)",
    )
    .execute(&mut **transaction.connection())
    .await
    .convert_error()?;
    db.post_relay_store()
        .report_capacity(transaction.connection(), report)
        .await?;
    let capacity = db
        .post_relay_store()
        .read_capacity(transaction.connection())
        .await?;
    transaction.commit().await?;
    pool.close().await;
    Ok(capacity)
}
