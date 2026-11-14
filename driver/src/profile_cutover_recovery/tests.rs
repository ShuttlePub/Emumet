use super::*;
use crate::database::isolated_db::IsolatedDatabase;
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::read_model::{
    AccountReadModel, DependOnAccountReadModel, DependOnProfileReadModel, ProfileReadModel,
};
use kernel::prelude::entity::{AccountId, AccountIsBot, AccountName, AuthAccountId, Nanoid};
use sqlx::Executor;
use std::collections::BTreeMap;

async fn account(iso: &IsolatedDatabase, dormant: bool) -> AccountId {
    kernel::ensure_generator_initialized();
    let id = AccountId::new(kernel::generate_id());
    let db = &iso.database;
    let mut con = db.connection().await.unwrap();
    let created = db
        .account_event_store()
        .persist_and_transform(
            &mut con,
            Account::create(
                id.clone(),
                AccountName::new(format!("a{}", id.as_ref())),
                AccountIsBot::new(false),
                Nanoid::new(format!("a{}", id.as_ref())),
                AuthAccountId::new(kernel::generate_id()),
            ),
        )
        .await
        .unwrap();
    let mut entity = None;
    Account::apply(&mut entity, created).unwrap();
    if dormant {
        let event = db
            .account_event_store()
            .persist_and_transform(
                &mut con,
                Account::deactivate(id.clone(), entity.as_ref().unwrap().version().clone()),
            )
            .await
            .unwrap();
        Account::apply(&mut entity, event).unwrap();
    }
    let entity = entity.unwrap();
    db.account_read_model()
        .create(&mut con, &entity)
        .await
        .unwrap();
    db.account_read_model()
        .update(&mut con, &entity)
        .await
        .unwrap();
    id
}

async fn profile(
    iso: &IsolatedDatabase,
    owner: &AccountId,
    projected: bool,
    username: Option<&str>,
) -> i64 {
    let id = kernel::generate_id();
    let db = &iso.database;
    let mut con = db.connection().await.unwrap();
    let event = db
        .profile_event_store()
        .persist_and_transform(
            &mut con,
            Profile::create_with_username(
                ProfileId::new(id),
                owner.clone(),
                None,
                None,
                None,
                None,
                Nanoid::new(format!("p{id}")),
                username.map(ProfileUsername::new),
            ),
        )
        .await
        .unwrap();
    let mut entity = None;
    Profile::apply(&mut entity, event).unwrap();
    if projected {
        db.profile_read_model()
            .create(&mut con, &entity.unwrap())
            .await
            .unwrap();
    }
    id
}

async fn request(iso: &IsolatedDatabase, id: i64, name: &str) -> UsernameRecoveryRequest {
    let version: i64 = sqlx::query_scalar("SELECT max(version) FROM profile_events WHERE id = $1")
        .bind(id)
        .fetch_one(&iso.pool)
        .await
        .unwrap();
    UsernameRecoveryRequest {
        request_id: format!("ticket-{id}"),
        operator: "test-operator".into(),
        reason: "owner approved permanent username".into(),
        profile_id: id,
        expected_version: version,
        username: name.into(),
    }
}

/// Includes every table, including migration state, old keys/federation, checkpoints and audit.
async fn snapshot(iso: &IsolatedDatabase) -> BTreeMap<String, String> {
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT tablename FROM pg_tables WHERE schemaname = 'public' ORDER BY 1",
    )
    .fetch_all(&iso.pool)
    .await
    .unwrap();
    let mut result = BTreeMap::new();
    for table in tables {
        let digest: String = sqlx::query_scalar(&format!(
            r#"SELECT COALESCE(md5(string_agg(to_jsonb(t)::text, '|' ORDER BY to_jsonb(t)::text)), '') FROM "{table}" t"#
        )).fetch_one(&iso.pool).await.unwrap();
        result.insert(table, digest);
    }
    result
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn old_dormant_transferred_profile_is_diagnosed_and_fixed_without_restoring_or_migrating() {
    let iso = IsolatedDatabase::create_before_cutover().await;
    let original_owner = account(&iso, false).await;
    let current_owner = account(&iso, true).await;
    let id = profile(&iso, &original_owner, false, None).await;
    let mut con = iso.database.connection().await.unwrap();
    iso.database
        .profile_event_store()
        .persist(
            &mut con,
            &Profile::transfer_account(ProfileId::new(id), original_owner, current_owner.clone()),
        )
        .await
        .unwrap();
    // Legacy pre-P4 lookup cannot see the stored deleted Account; the recovery must not use it.
    assert!(iso
        .database
        .account_read_model()
        .find_by_id_unfiltered(&mut con, &current_owner)
        .await
        .unwrap()
        .is_none());
    drop(con);
    // Put the old streams strictly outside BOTH tailing windows; do not run any new projector.
    iso.pool.execute("INSERT INTO projection_checkpoints (projector_name, last_seq, updated_at) VALUES ('account_projector', 10000, now()), ('profile_projector', 10000, now())").await.unwrap();
    let before = snapshot(&iso).await;
    let report = diagnose(&iso.pool).await.unwrap();
    assert_eq!(report.missing_usernames, 1);
    assert_eq!(report.problems, 0);
    assert_eq!(
        report.profiles[0].account_id,
        current_owner.as_ref().to_string()
    );
    assert_eq!(report.profiles[0].account_deactivated, Some(true));
    assert!(!report.profiles[0].profile_projection_exists);
    assert_eq!(snapshot(&iso).await, before, "diagnosis is read-only");
    assert!(
        iso.try_apply_reset().await.is_err(),
        "actual P4 refuses the missing username"
    );
    let req = request(&iso, id, "recovered-profile").await;
    let dry_run = set_username(&iso.pool, &req, false).await.unwrap();
    assert_eq!(dry_run.status, "dry_run");
    assert_eq!(dry_run.request_id, req.request_id);
    assert_eq!(dry_run.requested_username, req.username);
    assert_eq!(
        snapshot(&iso).await,
        before,
        "dry-run must not create even an audit table"
    );
    let applied = set_username(&iso.pool, &req, true).await.unwrap();
    assert_eq!(applied.status, "applied");
    let after = snapshot(&iso).await;
    for (table, digest) in before {
        if table != "profile_events" {
            assert_eq!(after[&table], digest, "{table} must be untouched");
        }
    }
    assert_eq!(
        set_username(&iso.pool, &req, true).await.unwrap().status,
        "already_applied"
    );
    assert_eq!(
        snapshot(&iso).await,
        after,
        "replay must not append an event/audit or regenerate keys"
    );
    assert_eq!(diagnose(&iso.pool).await.unwrap().missing_usernames, 0);
    iso.apply_reset().await;
    assert!(
        diagnose(&iso.pool).await.is_err(),
        "pre-P4-only command refuses an already cut-over DB"
    );
    assert!(set_username(&iso.pool, &req, true).await.is_err());
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn active_projection_and_audit_are_atomic_and_request_identity_is_immutable() {
    let iso = IsolatedDatabase::create_before_cutover().await;
    let owner = account(&iso, false).await;
    let id = profile(&iso, &owner, true, None).await;
    let req = request(&iso, id, "active-profile").await;
    let applied = set_username(&iso.pool, &req, true).await.unwrap();
    let (username, version): (String, i64) =
        sqlx::query_as("SELECT username, version FROM profiles WHERE id = $1")
            .bind(id)
            .fetch_one(&iso.pool)
            .await
            .unwrap();
    assert_eq!(username, req.username);
    assert_eq!(version.to_string(), applied.new_version.unwrap());
    let audit: serde_json::Value =
        sqlx::query_scalar("SELECT request FROM profile_cutover_recovery_audit")
            .fetch_one(&iso.pool)
            .await
            .unwrap();
    assert_eq!(audit, serde_json::to_value(&req).unwrap());
    let before = snapshot(&iso).await;
    let mut changed = req.clone();
    changed.reason = "changed".into();
    assert!(set_username(&iso.pool, &changed, true).await.is_err());
    changed = req;
    changed.request_id = "a-new-request".into();
    changed.expected_version = version;
    assert!(
        set_username(&iso.pool, &changed, true).await.is_err(),
        "username remains immutable even with the latest version"
    );
    assert_eq!(snapshot(&iso).await, before);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn pre_preview_audit_replay_enriches_output_without_rewriting_history() {
    let iso = IsolatedDatabase::create_before_cutover().await;
    let owner = account(&iso, true).await;
    let id = profile(&iso, &owner, false, None).await;
    let req = request(&iso, id, "previous-release").await;
    let applied = set_username(&iso.pool, &req, true).await.unwrap();
    sqlx::query(
        "UPDATE profile_cutover_recovery_audit
         SET result = result - 'request_id' - 'requested_username' WHERE request_id = $1",
    )
    .bind(&req.request_id)
    .execute(&iso.pool)
    .await
    .unwrap();
    let before = snapshot(&iso).await;
    let replay = set_username(&iso.pool, &req, true).await.unwrap();
    assert_eq!(replay.status, "already_applied");
    assert_eq!(replay.request_id, req.request_id);
    assert_eq!(replay.requested_username, req.username);
    assert_eq!(replay.new_version, applied.new_version);
    assert_eq!(snapshot(&iso).await, before);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn stale_version_and_reserved_dormant_username_are_refused_without_changes() {
    let iso = IsolatedDatabase::create_before_cutover().await;
    let owner = account(&iso, true).await;
    profile(&iso, &owner, false, Some("reserved-name")).await;
    let id = profile(&iso, &owner, false, None).await;
    let mut req = request(&iso, id, "available-name").await;
    let before = snapshot(&iso).await;
    req.expected_version -= 1;
    assert!(set_username(&iso.pool, &req, true).await.is_err());
    req.expected_version += 1;
    req.username = "reserved-name".into();
    assert!(set_username(&iso.pool, &req, true).await.is_err());
    assert_eq!(snapshot(&iso).await, before);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn concurrent_assignments_serialize_and_only_one_username_reservation_wins() {
    let iso = IsolatedDatabase::create_before_cutover().await;
    let owner = account(&iso, true).await;
    let a = profile(&iso, &owner, false, None).await;
    let b = profile(&iso, &owner, false, None).await;
    let a = request(&iso, a, "shared-name").await;
    let b = request(&iso, b, "shared-name").await;
    let (a, b) = tokio::join!(
        set_username(&iso.pool, &a, true),
        set_username(&iso.pool, &b, true)
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM profile_cutover_recovery_audit")
        .fetch_one(&iso.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn audit_failure_rolls_back_event_and_projection_together() {
    let iso = IsolatedDatabase::create_before_cutover().await;
    let owner = account(&iso, false).await;
    let id = profile(&iso, &owner, true, None).await;
    let req = request(&iso, id, "rollback-name").await;
    iso.pool.execute(AUDIT_DDL).await.unwrap();
    iso.pool.execute("CREATE FUNCTION refuse_audit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected audit failure'; END $$;
        CREATE TRIGGER refuse_audit BEFORE INSERT ON profile_cutover_recovery_audit FOR EACH ROW EXECUTE FUNCTION refuse_audit()").await.unwrap();
    let before = snapshot(&iso).await;
    let error = format!(
        "{:?}",
        set_username(&iso.pool, &req, true).await.unwrap_err()
    );
    assert!(error.contains("injected audit failure"), "{error}");
    assert_eq!(snapshot(&iso).await, before);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn row_only_and_inconsistent_history_fail_closed() {
    let iso = IsolatedDatabase::create_before_cutover().await;
    let owner = account(&iso, false).await;
    let id = profile(&iso, &owner, true, None).await;
    let req = request(&iso, id, "inconsistent").await;
    iso.pool
        .execute("UPDATE profiles SET version = version + 1")
        .await
        .unwrap();
    assert_eq!(diagnose(&iso.pool).await.unwrap().problems, 1);
    assert!(set_username(&iso.pool, &req, true).await.is_err());
    iso.pool
        .execute("DELETE FROM profile_events")
        .await
        .unwrap();
    let before = snapshot(&iso).await;
    let report = diagnose(&iso.pool).await.unwrap();
    assert_eq!(report.missing_usernames, 1);
    assert_eq!(report.problems, 1);
    assert!(set_username(&iso.pool, &req, true).await.is_err());
    assert_eq!(snapshot(&iso).await, before);
}

#[test]
fn validation_uses_the_same_username_policy_and_requires_attribution() {
    let mut req = UsernameRecoveryRequest {
        request_id: "request".into(),
        operator: "operator".into(),
        reason: "owner approved".into(),
        profile_id: 1,
        expected_version: 2,
        username: "a".repeat(64),
    };
    assert!(validate_request(&req).is_ok());
    for name in ["admin", "Alice", "", "a ", &"a".repeat(65)] {
        req.username = name.into();
        assert!(validate_request(&req).is_err(), "{name}");
    }
    req.username = "alice".into();
    req.reason = " \t".into();
    assert!(validate_request(&req).is_err());
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn diagnostic_and_dry_run_work_with_database_enforced_read_only_connections() {
    let iso = IsolatedDatabase::create_before_cutover().await;
    let owner = account(&iso, true).await;
    let id = profile(&iso, &owner, false, None).await;
    let req = request(&iso, id, "read-only-dry-run").await;
    let readonly = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .after_connect(|con, _| {
            Box::pin(async move {
                sqlx::query("SET default_transaction_read_only = on")
                    .execute(con)
                    .await?;
                Ok(())
            })
        })
        .connect_with(iso.pool.connect_options().as_ref().clone())
        .await
        .unwrap();
    let before = snapshot(&iso).await;
    assert_eq!(diagnose(&readonly).await.unwrap().missing_usernames, 1);
    assert_eq!(
        set_username(&readonly, &req, false).await.unwrap().status,
        "dry_run"
    );
    assert!(set_username(&readonly, &req, true).await.is_err());
    assert_eq!(snapshot(&iso).await, before);
    readonly.close().await;
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn invalid_schema_and_malformed_history_refuse_without_side_effects() {
    let iso = IsolatedDatabase::create_before_cutover().await;
    let owner = account(&iso, true).await;
    let id = profile(&iso, &owner, false, None).await;
    let req = request(&iso, id, "fail-closed").await;
    sqlx::query(
        "UPDATE profile_events SET data = jsonb_set(data, '{type}', '\"unknown\"') WHERE id = $1",
    )
    .bind(id)
    .execute(&iso.pool)
    .await
    .unwrap();
    let before = snapshot(&iso).await;
    assert!(diagnose(&iso.pool).await.is_err());
    assert!(set_username(&iso.pool, &req, true).await.is_err());
    assert_eq!(snapshot(&iso).await, before);
    iso.pool
        .execute("DROP INDEX profile_events_username_unique")
        .await
        .unwrap();
    let before = snapshot(&iso).await;
    assert!(diagnose(&iso.pool).await.is_err());
    assert!(set_username(&iso.pool, &req, true).await.is_err());
    assert_eq!(snapshot(&iso).await, before);
}
