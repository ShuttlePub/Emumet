//! Profile social graph の実 DB テスト (driver / application / server) が使う、使い捨ての独立 DB。
//!
//! `cfg(test)` または `test-mode` ビルドにだけ存在する。
//!
//! テストごとに `CREATE DATABASE` で専用 DB を作り、本番 migrations を適用する。
//! 共有の `DATABASE_URL` の DB (旧 schema のテストが使う) は変更しない。DB は `Drop` で削除する。
//!
//! P4 で Profile social graph の schema と旧連合データの初期化は本番 migrations に入った。
//! `create()` は全 migration 適用済み (初期化は空の DB に対して実行済み)。初期化の前後を検証する
//! テストは `create_before_reset()` で初期化 migration の直前の DB を作り、データを入れてから
//! `apply_reset()` (= 残りの本番 migration の適用) を行う。

use crate::database::env;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{Connection, Executor, PgConnection, Pool, Postgres};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};

use super::PostgresDatabase;

/// 全既存 Profile の前提検査を含む、最初の切替 migration のバージョン。
pub const CUTOVER_FIRST_MIGRATION_VERSION: i64 = 20261120000000;
/// 初期化 migration のバージョン。これより前の migration が「初期化直前」の DB を作る。
pub const RESET_MIGRATION_VERSION: i64 = 20261120000002;
pub const RESET_SQL: &str =
    include_str!("../../../../migrations/20261120000002_reset_legacy_federation.sql");

static COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct IsolatedDatabase {
    pub database: PostgresDatabase,
    pub pool: Pool<Postgres>,
    name: String,
    admin_options: PgConnectOptions,
}

impl IsolatedDatabase {
    /// 全ての本番 migrations を適用した独立 DB (初期化は空の DB に対して実行済み)。
    pub async fn create() -> Self {
        Self::create_with(None).await
    }

    /// P4 の migration を **1 つも適用していない**独立 DB (切替前の本番の状態)。
    pub async fn create_before_cutover() -> Self {
        Self::create_with(Some(CUTOVER_FIRST_MIGRATION_VERSION)).await
    }

    /// 旧連合データの初期化 migration の**直前**まで (新 Profile schema は適用済み) の独立 DB。
    /// 旧データを入れてから [`Self::apply_reset`] で切替を実行する。
    pub async fn create_before_reset() -> Self {
        Self::create_with(Some(RESET_MIGRATION_VERSION)).await
    }

    async fn create_with(stop_before: Option<i64>) -> Self {
        let url = env("DATABASE_URL")
            .unwrap()
            .expect("DATABASE_URL must be set for the isolated database tests");
        let admin_options = PgConnectOptions::from_str(&url).unwrap();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let name = format!(
            "emumet_p2_{}_{}_{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
            nanos % 1_000_000_000
        );
        let mut admin = PgConnection::connect_with(&admin_options).await.unwrap();
        admin
            .execute(format!("CREATE DATABASE \"{name}\"").as_str())
            .await
            .unwrap();
        admin.close().await.unwrap();

        let pool = PgPoolOptions::new()
            .max_connections(16)
            .connect_with(admin_options.clone().database(&name))
            .await
            .unwrap();
        let full = sqlx::migrate!("../migrations");
        match stop_before {
            None => full.run(&pool).await.unwrap(),
            Some(version) => {
                let partial = sqlx::migrate::Migrator {
                    migrations: std::borrow::Cow::Owned(
                        full.migrations
                            .iter()
                            .filter(|migration| migration.version < version)
                            .cloned()
                            .collect(),
                    ),
                    ignore_missing: false,
                    locking: true,
                };
                partial.run(&pool).await.unwrap();
            }
        }
        let database = PostgresDatabase::from_pool(pool.clone());
        Self {
            database,
            pool,
            name,
            admin_options,
        }
    }

    /// 残りの本番 migrations (= 旧連合データの初期化) を適用する。失敗は `Err` で返す。
    pub async fn try_apply_reset(&self) -> Result<(), sqlx::migrate::MigrateError> {
        let mut connection = self.pool.acquire().await?;
        let result = PostgresDatabase::migrate(&mut connection).await;
        if result.is_err() {
            // sqlx 0.7 は失敗時に migration の advisory lock を解放せずに接続を返すため、
            // 後続の migrate が待ち続けないよう、この接続の lock を明示的に解放する。
            let _ = connection.execute("SELECT pg_advisory_unlock_all()").await;
        }
        result
    }

    pub async fn apply_reset(&self) {
        self.try_apply_reset().await.unwrap();
    }

    /// 初期化 SQL そのものを再実行する (台帳があれば何も消さないことの検証用)。
    pub async fn rerun_reset_sql(&self) {
        self.pool.execute(RESET_SQL).await.unwrap();
    }
}

impl Drop for IsolatedDatabase {
    fn drop(&mut self) {
        let name = self.name.clone();
        let options = self.admin_options.clone();
        let _ = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                if let Ok(mut admin) = PgConnection::connect_with(&options).await {
                    let _ = admin
                        .execute(
                            format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)").as_str(),
                        )
                        .await;
                }
            });
        })
        .join();
    }
}
