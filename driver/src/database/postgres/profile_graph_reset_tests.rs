//! 旧連合データだけを初期化する本番 migration (20261120000002_reset_legacy_federation.sql) の実 DB テスト。
//! 独立 DB で「初期化直前の DB に旧データを入れ、残りの本番 migration を適用する」実切替を再現し、
//! 初期化の前後に対象外データが一切変わらないことを照合する。

use super::profile_graph_test_support::{seed_personal_profile, IsolatedDatabase, RESET_SQL};
use super::PostgresDatabase;
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::repository::{
    BeginFollowOutcome, DependOnProfileBlockRepository, DependOnProfileDeliveryEnqueuer,
    DependOnProfileFollowRepository, ProfileBlockRepository, ProfileDeliveryEnqueuer,
    ProfileFollowRepository,
};
use kernel::prelude::entity::{
    DeliveryActivityType, GraphActivityId, GraphActorUrl, GraphInboxUrl, NewProfileDeliveryJob,
    ProfileBlock, ProfileBlockId, ProfileFollow, ProfileFollowId, ProfileGraphActor, ProfileId,
};
use sqlx::{Executor, Pool, Postgres};
use std::collections::BTreeMap;
use time::OffsetDateTime;

/// 初期化対象。migration の TRUNCATE と一致していなければならない。
const RESET_TARGETS: [&str; 6] = [
    "follows",
    "blocks",
    "mutes",
    "remote_accounts",
    "outbox_activities",
    "signing_keys",
];

const LEDGER: &str = "profile_federation_resets";

type Snapshot = BTreeMap<String, (i64, String)>;

async fn table_names(pool: &Pool<Postgres>) -> Vec<String> {
    sqlx::query_scalar("SELECT tablename FROM pg_tables WHERE schemaname = 'public' ORDER BY 1")
        .fetch_all(pool)
        .await
        .unwrap()
}

/// 初期化対象と台帳を除く全テーブルの (行数, 内容の hash)。将来の新テーブルも自動的に対象外として検査される。
/// NULL の列は hash から除く (後続の migration が足す nullable 列で、保持した行の内容が変わったことにしない)。
async fn retained_snapshot(pool: &Pool<Postgres>) -> Snapshot {
    let mut snapshot = Snapshot::new();
    for table in table_names(pool).await {
        if RESET_TARGETS.contains(&table.as_str()) || table == LEDGER || table == "_sqlx_migrations"
        {
            continue;
        }
        let (count, digest): (i64, String) = sqlx::query_as(&format!(
            "SELECT count(*)::bigint, coalesce(md5(string_agg(jsonb_strip_nulls(to_jsonb(t))::text, '|' \
                                         ORDER BY jsonb_strip_nulls(to_jsonb(t))::text)), '') \
             FROM \"{table}\" t"
        ))
        .fetch_one(pool)
        .await
        .unwrap();
        snapshot.insert(table, (count, digest));
    }
    snapshot
}

async fn count(pool: &Pool<Postgres>, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT count(*)::bigint FROM \"{table}\""))
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn seed_legacy_federation(pool: &Pool<Postgres>, account_id: i64) {
    let image_id = kernel::generate_id();
    sqlx::query("INSERT INTO images (id, url, hash, blurhash) VALUES ($1, $2, 'h', 'b')")
        .bind(image_id)
        .bind(format!("https://img.example/{image_id}"))
        .execute(pool)
        .await
        .unwrap();
    let remote_id = kernel::generate_id();
    sqlx::query(
        "INSERT INTO remote_accounts (id, acct, url, icon_id, inbox_url, public_key_pem) \
         VALUES ($1, $2, $3, $4, 'https://r.example/inbox', 'pem')",
    )
    .bind(remote_id)
    .bind(format!("bob{remote_id}@r.example"))
    .bind(format!("https://r.example/users/{remote_id}"))
    .bind(image_id)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO follows (id, follower_local_id, followee_remote_id, approved_at) \
         VALUES ($1, $2, $3, now())",
    )
    .bind(kernel::generate_id())
    .bind(account_id)
    .bind(remote_id)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO blocks (id, blocker_local_id, blocked_remote_id) VALUES ($1, $2, $3)")
        .bind(kernel::generate_id())
        .bind(account_id)
        .bind(remote_id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO mutes (id, muter_local_id, muted_remote_id) VALUES ($1, $2, $3)")
        .bind(kernel::generate_id())
        .bind(account_id)
        .bind(remote_id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO outbox_activities (account_id, activity_id, activity_type, object_json) \
         VALUES ($1, $2, 'Follow', '{}')",
    )
    .bind(account_id)
    .bind(format!(
        "https://local.example/activities/{}",
        kernel::generate_id()
    ))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO signing_keys (id, account_id, algorithm, encrypted_private_key, public_key_pem, key_id_uri) \
         VALUES ($1, $2, 'rsa', '{}'::jsonb, 'pem', 'https://local.example/key')",
    )
    .bind(kernel::generate_id())
    .bind(account_id)
    .execute(pool)
    .await
    .unwrap();
}

/// 対象外データ: 認証・Account・Profile・組織 membership・イベント履歴・全体モデレーション・
/// 新モデル (Profile 版 Follow/block/配送 job)。
async fn seed_retained(
    database: &PostgresDatabase,
    pool: &Pool<Postgres>,
    with_new_model: bool,
) -> i64 {
    let (account_id, profile_id) = seed_personal_profile(database).await;
    let (other_account_id, other_profile_id) = seed_personal_profile(database).await;
    let account = *account_id.as_ref();
    let other_account = *other_account_id.as_ref();

    let host_id = kernel::generate_id();
    let auth_id = kernel::generate_id();
    sqlx::query("INSERT INTO auth_hosts (id, url) VALUES ($1, $2)")
        .bind(host_id)
        .bind(format!("https://auth.example/{host_id}"))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO auth_accounts (id, host_id, client_id) VALUES ($1, $2, 'client')")
        .bind(auth_id)
        .bind(host_id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO auth_emumet_accounts (emumet_id, auth_id) VALUES ($1, $2)")
        .bind(account)
        .bind(auth_id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO organization_members (org_account_id, member_account_id, role, status, invited_by) \
         VALUES ($1, $2, 'owner', 'active', $2)",
    )
    .bind(other_account)
    .bind(account)
    .execute(pool)
    .await
    .unwrap();
    for table in ["account_events", "profile_events", "account_report_events"] {
        sqlx::query(&format!(
            "INSERT INTO {table} (id, version, event_name, data) VALUES ($1, 1, 'created', '{{}}'::jsonb)"
        ))
        .bind(kernel::generate_id())
        .execute(pool)
        .await
        .unwrap();
    }
    sqlx::query(
        "INSERT INTO account_reports (id, target_account_id, reported_by_account_id, category, version, nanoid) \
         VALUES ($1, $2, $3, 'spam', 1, $4)",
    )
    .bind(kernel::generate_id())
    .bind(other_account)
    .bind(account)
    .bind(nanoid::nanoid!())
    .execute(pool)
    .await
    .unwrap();

    if with_new_model {
        let bob =
            ProfileGraphActor::Remote(GraphActorUrl::new("https://r.example/users/new").unwrap());
        let me = ProfileGraphActor::Local(profile_id.clone());
        let follow = ProfileFollow::new_accepted(
            ProfileFollowId::new(kernel::generate_id()),
            me,
            bob.clone(),
            GraphActivityId::new("https://local.example/activities/new-follow").unwrap(),
            OffsetDateTime::now_utc(),
        )
        .unwrap();
        let mut conn = database.connection().await.unwrap();
        assert_eq!(
            database
                .profile_follow_repository()
                .begin_generation(&mut conn, &follow)
                .await
                .unwrap(),
            BeginFollowOutcome::Created
        );
        let block = ProfileBlock::new(
            ProfileBlockId::new(kernel::generate_id()),
            other_profile_id.clone(),
            bob,
            OffsetDateTime::now_utc(),
        )
        .unwrap();
        database
            .profile_block_repository()
            .insert_if_absent(&mut conn, &block)
            .await
            .unwrap();
        let job = NewProfileDeliveryJob::new(
            profile_id,
            DeliveryActivityType::Follow,
            GraphActivityId::new("https://local.example/activities/new-follow").unwrap(),
            GraphInboxUrl::new("https://r.example/inbox").unwrap(),
            "{\"type\":\"Follow\"}",
            OffsetDateTime::now_utc(),
        )
        .unwrap()
        .with_follow(follow.id().clone());
        database
            .profile_delivery_enqueuer()
            .enqueue(&mut conn, &job)
            .await
            .unwrap();
    }
    assign_usernames(pool).await;
    account
}

/// 切替の前提 (全 Profile が username 設定済み) を満たす。
async fn assign_usernames(pool: &Pool<Postgres>) {
    sqlx::query("UPDATE profiles SET username = 'seed-' || id WHERE username IS NULL")
        .execute(pool)
        .await
        .unwrap();
}

fn executable_sql(sql: &str) -> String {
    sql.lines()
        .map(|line| line.split("--").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase()
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn reset_clears_only_legacy_federation_data() {
    kernel::ensure_generator_initialized();
    let iso = IsolatedDatabase::create_before_reset().await;
    let account = seed_retained(&iso.database, &iso.pool, true).await;
    seed_legacy_federation(&iso.pool, account).await;
    for table in RESET_TARGETS {
        assert!(count(&iso.pool, table).await > 0, "{table} must be seeded");
    }
    let before = retained_snapshot(&iso.pool).await;
    for table in [
        "accounts",
        "profiles",
        "auth_hosts",
        "auth_accounts",
        "auth_emumet_accounts",
        "organization_members",
        "account_events",
        "profile_events",
        "account_report_events",
        "account_reports",
        "images",
        "profile_follows",
        "profile_blocks",
        "profile_delivery_jobs",
    ] {
        assert!(before[table].0 > 0, "{table} must hold retained data");
    }

    iso.apply_reset().await;

    for table in RESET_TARGETS {
        assert_eq!(count(&iso.pool, table).await, 0, "{table} must be emptied");
    }
    // `apply_reset` applies every remaining production migration, so migrations numbered after the
    // reset (new tables of later features) may add tables. Each table that existed before must be
    // unchanged; a table added by a later migration starts empty.
    let after = retained_snapshot(&iso.pool).await;
    let after_existing: Snapshot = after
        .iter()
        .filter(|(table, _)| before.contains_key(*table))
        .map(|(table, state)| (table.clone(), state.clone()))
        .collect();
    assert_eq!(before, after_existing, "no retained table may change");
    for (table, (rows, _)) in after.iter().filter(|(t, _)| !before.contains_key(*t)) {
        assert_eq!(
            *rows, 0,
            "{table} was added by a later migration and starts empty"
        );
    }
    assert_eq!(count(&iso.pool, LEDGER).await, 1);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn cutover_is_refused_while_a_profile_has_no_username() {
    kernel::ensure_generator_initialized();
    let iso = IsolatedDatabase::create_before_reset().await;
    let account = seed_retained(&iso.database, &iso.pool, true).await;
    seed_legacy_federation(&iso.pool, account).await;
    let (_, unnamed) = seed_personal_profile(&iso.database).await;
    let before_retained = retained_snapshot(&iso.pool).await;
    let before_legacy: Vec<i64> = {
        let mut counts = Vec::new();
        for table in RESET_TARGETS {
            counts.push(count(&iso.pool, table).await);
        }
        counts
    };
    assert!(before_legacy.iter().all(|c| *c > 0));

    let refused = iso.try_apply_reset().await;
    let message = format!(
        "{:?}",
        refused.expect_err("a Profile without username must refuse")
    );
    assert!(
        message.contains("no username"),
        "the refusal must say why: {message}"
    );

    // 拒否は何も変えない: 旧データも保持データも台帳も migration 記録も残らない。
    let after_legacy: Vec<i64> = {
        let mut counts = Vec::new();
        for table in RESET_TARGETS {
            counts.push(count(&iso.pool, table).await);
        }
        counts
    };
    assert_eq!(before_legacy, after_legacy);
    assert_eq!(before_retained, retained_snapshot(&iso.pool).await);
    assert!(!table_names(&iso.pool).await.contains(&LEDGER.to_string()));
    let applied: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM _sqlx_migrations WHERE version = 20261120000002",
    )
    .fetch_one(&iso.pool)
    .await
    .unwrap();
    assert_eq!(applied, 0);

    // username を設定すれば切替できる。
    sqlx::query("UPDATE profiles SET username = 'late-name' WHERE id = $1")
        .bind(*unnamed.as_ref())
        .execute(&iso.pool)
        .await
        .unwrap();
    iso.apply_reset().await;
    for table in RESET_TARGETS {
        assert_eq!(count(&iso.pool, table).await, 0);
    }
    assert_eq!(count(&iso.pool, LEDGER).await, 1);
}

/// 本番と同じ「P4 migration を 1 つも適用していない」DB で username 未設定の Profile が残っていると、
/// 最初の P4 migration が拒否し、新 schema も作られず DB は切替前のまま (旧バイナリで復旧できる)。
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn precheck_refuses_before_any_p4_migration_is_applied() {
    kernel::ensure_generator_initialized();
    let iso = IsolatedDatabase::create_before_cutover().await;
    let account = seed_retained(&iso.database, &iso.pool, false).await;
    seed_legacy_federation(&iso.pool, account).await;
    let (_, unnamed) = seed_personal_profile(&iso.database).await;
    let before_retained = retained_snapshot(&iso.pool).await;
    let tables_before = table_names(&iso.pool).await;

    let message = format!(
        "{:?}",
        iso.try_apply_reset()
            .await
            .expect_err("a Profile without username must refuse the cutover")
    );
    assert!(
        message.contains("no username"),
        "the refusal must say why: {message}"
    );

    // 何も適用されない: 新 schema も台帳もなく、旧データ・保持データも変わらない。
    assert_eq!(tables_before, table_names(&iso.pool).await);
    assert!(!table_names(&iso.pool)
        .await
        .contains(&"profile_follows".to_string()));
    let applied: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM _sqlx_migrations WHERE version >= 20261120000001",
    )
    .fetch_one(&iso.pool)
    .await
    .unwrap();
    assert_eq!(applied, 0);
    for table in RESET_TARGETS {
        assert!(
            count(&iso.pool, table).await > 0,
            "{table} must be untouched"
        );
    }
    assert_eq!(before_retained, retained_snapshot(&iso.pool).await);

    // 旧バイナリ相当で username を設定すれば、そのまま切替できる。
    sqlx::query("UPDATE profiles SET username = 'late-name' WHERE id = $1")
        .bind(*unnamed.as_ref())
        .execute(&iso.pool)
        .await
        .unwrap();
    iso.apply_reset().await;
    for table in RESET_TARGETS {
        assert_eq!(count(&iso.pool, table).await, 0);
    }
    assert_eq!(count(&iso.pool, LEDGER).await, 1);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn reset_is_not_repeated() {
    kernel::ensure_generator_initialized();
    let iso = IsolatedDatabase::create_before_reset().await;
    let account = seed_retained(&iso.database, &iso.pool, true).await;
    seed_legacy_federation(&iso.pool, account).await;
    iso.apply_reset().await;

    // 初期化後に作られたデータ (旧テーブルへの書込を含む) は再実行で消えない。
    seed_legacy_federation(&iso.pool, account).await;
    let (_, extra_profile) = seed_personal_profile(&iso.database).await;
    let _: ProfileId = extra_profile;
    // The settle bound of an individual block (a later feature's table) is retained data: a repeated
    // reset run must leave it alone (it is part of the retained snapshot compared below).
    sqlx::query(
        "INSERT INTO profile_block_settle (sender_profile_id, target_actor_url, settle_not_before) \
         VALUES ($1, 'https://remote.example/users/bob', now() + interval '10 minutes')",
    )
    .bind(extra_profile.as_ref())
    .execute(&iso.pool)
    .await
    .unwrap();
    assign_usernames(&iso.pool).await;
    let before = retained_snapshot(&iso.pool).await;
    let legacy_counts: Vec<i64> = {
        let mut counts = Vec::new();
        for table in RESET_TARGETS {
            counts.push(count(&iso.pool, table).await);
        }
        counts
    };
    assert!(legacy_counts.iter().all(|c| *c > 0));

    iso.apply_reset().await;
    iso.rerun_reset_sql().await;
    iso.rerun_reset_sql().await;

    let after_counts: Vec<i64> = {
        let mut counts = Vec::new();
        for table in RESET_TARGETS {
            counts.push(count(&iso.pool, table).await);
        }
        counts
    };
    assert_eq!(
        legacy_counts, after_counts,
        "a repeated run must not erase again"
    );
    assert_eq!(before, retained_snapshot(&iso.pool).await);
    assert_eq!(count(&iso.pool, "profile_block_settle").await, 1);
    assert_eq!(count(&iso.pool, LEDGER).await, 1);
}

#[test]
fn reset_migration_never_uses_cascade_and_lists_exact_targets() {
    let sql = executable_sql(RESET_SQL);
    assert!(
        !sql.contains("cascade"),
        "TRUNCATE ... CASCADE is forbidden"
    );
    assert!(
        !sql.contains("drop table"),
        "the reset must not drop tables"
    );
    let truncates: Vec<&str> = sql
        .split(';')
        .map(str::trim)
        .filter(|statement| statement.contains("truncate"))
        .collect();
    assert_eq!(truncates.len(), 1);
    let list = truncates[0]
        .split("truncate table")
        .nth(1)
        .expect("explicit TRUNCATE TABLE list");
    let mut listed: Vec<&str> = list.split(',').map(str::trim).collect();
    let mut expected: Vec<&str> = RESET_TARGETS.to_vec();
    listed.sort();
    expected.sort();
    assert_eq!(listed, expected);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn no_retained_table_references_a_reset_target() {
    let iso = IsolatedDatabase::create().await;
    let targets: Vec<String> = RESET_TARGETS.iter().map(|t| t.to_string()).collect();
    let dependents: Vec<(String, String)> = sqlx::query_as(
        "SELECT conrelid::regclass::text, confrelid::regclass::text FROM pg_constraint \
         WHERE contype = 'f' AND confrelid::regclass::text = ANY($1) \
           AND NOT (conrelid::regclass::text = ANY($1))",
    )
    .bind(&targets)
    .fetch_all(&iso.pool)
    .await
    .unwrap();
    assert!(
        dependents.is_empty(),
        "retained tables must not depend on reset targets: {dependents:?}"
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn reset_fails_instead_of_cascading_into_a_dependent_table() {
    kernel::ensure_generator_initialized();
    let iso = IsolatedDatabase::create_before_reset().await;
    let account = seed_retained(&iso.database, &iso.pool, false).await;
    seed_legacy_federation(&iso.pool, account).await;
    iso.pool
        .execute(
            "CREATE TABLE future_dependent (remote_id BIGINT REFERENCES remote_accounts (id) ON DELETE CASCADE)",
        )
        .await
        .unwrap();
    let remote_id: i64 = sqlx::query_scalar("SELECT id FROM remote_accounts LIMIT 1")
        .fetch_one(&iso.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO future_dependent (remote_id) VALUES ($1)")
        .bind(remote_id)
        .execute(&iso.pool)
        .await
        .unwrap();
    let before_legacy = count(&iso.pool, "follows").await;

    assert!(iso.try_apply_reset().await.is_err());

    // 失敗は全体が rollback され、何も消えず、台帳にも残らない。
    assert_eq!(count(&iso.pool, "future_dependent").await, 1);
    assert_eq!(count(&iso.pool, "follows").await, before_legacy);
    assert_eq!(count(&iso.pool, "remote_accounts").await, 1);
    assert!(
        !table_names(&iso.pool).await.contains(&LEDGER.to_string()),
        "the failed run must leave no ledger behind"
    );
}
