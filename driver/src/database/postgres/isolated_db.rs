//! Profile social graph の実 DB テスト (driver / application / server) が使う、使い捨ての独立 DB。
//!
//! `cfg(test)` または `test-mode` ビルドにだけ存在する。
//!
//! テストごとに `CREATE DATABASE` で専用 DB を作り、本番 migrations と P2 の schema fixture を適用する。
//! 共有の `DATABASE_URL` の DB (旧 schema のテストが使う) は変更しない。DB は `Drop` で削除する。

use crate::database::env;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{Connection, Executor, PgConnection, Pool, Postgres};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};

use super::PostgresDatabase;

pub const SCHEMA_SQL: &str =
    include_str!("../../../fixtures/profile_social_graph/01_profile_social_graph_schema.sql");
pub const RESET_SQL: &str =
    include_str!("../../../fixtures/profile_social_graph/02_reset_legacy_federation.sql");

static COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct IsolatedDatabase {
    pub database: PostgresDatabase,
    pub pool: Pool<Postgres>,
    name: String,
    admin_options: PgConnectOptions,
}

impl IsolatedDatabase {
    /// 本番 migrations だけを適用した独立 DB (P2 の schema fixture は未適用)。
    pub async fn create_legacy() -> Self {
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
        sqlx::migrate!("../migrations").run(&pool).await.unwrap();
        let database = PostgresDatabase::from_pool(pool.clone());
        Self {
            database,
            pool,
            name,
            admin_options,
        }
    }

    /// 本番 migrations と P2 の schema fixture を適用した独立 DB。
    pub async fn create() -> Self {
        let isolated = Self::create_legacy().await;
        isolated.apply_schema().await;
        isolated
    }

    pub async fn apply_schema(&self) {
        self.pool.execute(SCHEMA_SQL).await.unwrap();
    }

    pub async fn apply_reset(&self) {
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
