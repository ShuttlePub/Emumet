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
