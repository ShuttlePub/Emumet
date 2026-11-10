//! Profile social graph の実 DB テストが使う、使い捨ての独立 DB。
//!
//! テストごとに `CREATE DATABASE` で専用 DB を作り、本番 migrations と P2 の schema fixture を適用する。
//! 共有の `DATABASE_URL` の DB (旧 schema のテストが使う) は変更しない。DB は `Drop` で削除する。

use crate::database::env;
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::read_model::{
    AccountReadModel, DependOnAccountReadModel, DependOnProfileReadModel, ProfileReadModel,
};
use kernel::prelude::entity::{AccountId, AccountKind, ProfileId};
use kernel::test_utils::{AccountBuilder, ProfileBuilder};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{Connection, Executor, PgConnection, Pool, Postgres};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};

use super::PostgresDatabase;

pub(crate) const SCHEMA_SQL: &str =
    include_str!("../../../fixtures/profile_social_graph/01_profile_social_graph_schema.sql");
pub(crate) const RESET_SQL: &str =
    include_str!("../../../fixtures/profile_social_graph/02_reset_legacy_federation.sql");

static COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) struct IsolatedDatabase {
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

/// 個人 Account とその Profile を作る。
pub(crate) async fn seed_personal_profile(database: &PostgresDatabase) -> (AccountId, ProfileId) {
    let account_id = AccountId::default();
    let account = AccountBuilder::new().id(account_id.clone()).build();
    seed_profile_on(database, account, &account_id).await
}

/// 組織 Account (同一 Account に複数 Profile を持てる) を作る。
pub(crate) async fn seed_organization_account(database: &PostgresDatabase) -> AccountId {
    let account_id = AccountId::default();
    let account = AccountBuilder::new()
        .id(account_id.clone())
        .kind(AccountKind::Organization)
        .build();
    let mut conn = database.connection().await.unwrap();
    database
        .account_read_model()
        .create(&mut conn, &account)
        .await
        .unwrap();
    account_id
}

/// 既存 Account に Profile を 1 つ追加する (組織 Account では複数可)。
pub(crate) async fn seed_profile_for(
    database: &PostgresDatabase,
    account_id: &AccountId,
) -> ProfileId {
    let profile_id = ProfileId::new(kernel::generate_id());
    let profile = ProfileBuilder::new()
        .id(profile_id.clone())
        .account_id(account_id.clone())
        .build();
    let mut conn = database.connection().await.unwrap();
    database
        .profile_read_model()
        .create(&mut conn, &profile)
        .await
        .unwrap();
    profile_id
}

async fn seed_profile_on(
    database: &PostgresDatabase,
    account: kernel::prelude::entity::Account,
    account_id: &AccountId,
) -> (AccountId, ProfileId) {
    let mut conn = database.connection().await.unwrap();
    database
        .account_read_model()
        .create(&mut conn, &account)
        .await
        .unwrap();
    drop(conn);
    let profile_id = seed_profile_for(database, account_id).await;
    (account_id.clone(), profile_id)
}
