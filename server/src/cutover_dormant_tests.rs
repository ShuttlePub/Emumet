//! Pre-cutover deactivation must not hide an existing Profile from the cutover.
use super::*;
use application::projection::{ProjectAccountBatch, ProjectProfileBatch};
use application::service::account::{DeactivateAccountUseCase, ReactivateAccountUseCase};
use kernel::interfaces::repository::{
    AggregateRepository, DependOnAccountRepository, DependOnProfileRepository,
};
use kernel::prelude::entity::{AccountIsBot, ProfileUsername};

const PRECHECK_VERSION: i64 = 20261120000000;

async fn project(r: &Rehearsal) {
    for _ in 0..4 {
        r.module.project_batch().await.unwrap();
        r.module.project_profile_batch().await.unwrap();
    }
}

/// Pre-P4 creation: persist real events and projection rows, deliberately WITHOUT generating
/// keys (the post-P4 creation use case would already generate one and hide this regression).
async fn seed_history(r: &Rehearsal, username: Option<&str>, set_later: bool) -> Local {
    let local = r
        .seed_local(username.unwrap_or("temporary-projection-name"))
        .await;
    let mut conn = r.module.database_connection().connection().await.unwrap();
    r.module
        .account_repository()
        .save(
            &mut conn,
            Account::create(
                local.account.id().clone(),
                local.account.name().clone(),
                AccountIsBot::new(false),
                local.account.nanoid().clone(),
                local.auth_account_id.clone(),
            ),
        )
        .await
        .unwrap();
    r.module
        .profile_repository()
        .save(
            &mut conn,
            Profile::create_with_username(
                local.profile.id().clone(),
                local.account.id().clone(),
                None,
                None,
                None,
                None,
                local.profile.nanoid().clone(),
                username.filter(|_| !set_later).map(ProfileUsername::new),
            ),
        )
        .await
        .unwrap();
    if let Some(username) = username.filter(|_| set_later) {
        r.module
            .profile_repository()
            .save(
                &mut conn,
                Profile::set_username(local.profile.id().clone(), ProfileUsername::new(username)),
            )
            .await
            .unwrap();
    }
    // Match the immediate pre-P4 projection write to the actual event versions. Builder
    // versions are unrelated generated ids; folding a Created onto such a row would be invalid.
    let account = r
        .module
        .account_repository()
        .load(&mut conn, local.account.id())
        .await
        .unwrap();
    r.module
        .account_read_model()
        .update(&mut conn, account.aggregate())
        .await
        .unwrap();
    let profile = r
        .module
        .profile_repository()
        .load(&mut conn, local.profile.id())
        .await
        .unwrap();
    r.module
        .profile_read_model()
        .update(&mut conn, profile.aggregate())
        .await
        .unwrap();
    drop(conn);
    project(r).await;
    local
}

async fn deactivate(r: &Rehearsal, local: &Local) {
    r.module
        .deactivate_account(
            &local.auth_account_id,
            local.account.nanoid().as_ref().clone(),
        )
        .await
        .unwrap();
    project(r).await;
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM profiles WHERE id = $1")
        .bind(local.profile.id().as_ref())
        .fetch_one(&r.iso.pool)
        .await
        .unwrap();
    assert_eq!(
        rows, 0,
        "real Account deactivation must remove the projection"
    );
}

async fn reactivate(r: &Rehearsal, local: &Local) {
    r.module
        .reactivate_account(
            &local.auth_account_id,
            local.account.nanoid().as_ref().clone(),
        )
        .await
        .unwrap();
    project(r).await;
}

async fn dormant_cutover(set_later: bool) {
    let r = Rehearsal::new().await;
    let local = seed_history(&r, Some("dormant-profile"), set_later).await;
    deactivate(&r, &local).await;
    r.seed_legacy_federation(&local).await;
    assert!(r.key_rows().await.is_empty(), "no pre-cutover Profile key");
    let before = r.retained_snapshot().await;

    // These are the production startup steps, with the Account still deactivated.
    r.iso.apply_reset().await;
    assert_eq!(provision_profile_keys(&r.module).await.unwrap().created, 1);
    let keys = r.key_rows().await;
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].1, *local.profile.id().as_ref());
    assert_eq!(only_tables_of(r.retained_snapshot().await, &before), before);
    assert_eq!(r.legacy_counts().await, vec![0; RESET_TARGETS.len()]);
    let uri = format!("/ap/profiles/{}", local.profile.nanoid().as_ref());
    assert_eq!(
        r.get_json(&uri).await.0,
        StatusCode::NOT_FOUND,
        "key generation must not reactivate or publish the disabled Profile"
    );
    assert_eq!(provision_profile_keys(&r.module).await.unwrap().created, 0);
    assert_eq!(r.key_rows().await, keys);

    reactivate(&r, &local).await;
    // No restart and NO key provisioning between reactivation and this request.
    let (status, actor) = r.get_json(&uri).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "reactivated Profile has its actor key immediately"
    );
    assert_eq!(actor["id"], actor_url(&r.module, &local.profile));
    assert_eq!(actor["preferredUsername"], "dormant-profile");
    assert_eq!(actor["publicKey"]["publicKeyPem"], keys[0].3);
    assert_eq!(
        r.key_rows().await,
        keys,
        "reactivation does not regenerate keys"
    );
    let identity: (i64, String, String) =
        sqlx::query_as("SELECT id, nanoid, username FROM profiles WHERE id = $1")
            .bind(local.profile.id().as_ref())
            .fetch_one(&r.iso.pool)
            .await
            .unwrap();
    assert_eq!(
        identity,
        (
            *local.profile.id().as_ref(),
            local.profile.nanoid().as_ref().clone(),
            "dormant-profile".to_string()
        )
    );
    r.iso.apply_reset().await;
    assert_eq!(provision_profile_keys(&r.module).await.unwrap().created, 0);
    assert_eq!(r.key_rows().await, keys);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn pre_cutover_deactivation_then_cutover_and_reactivation_has_a_key_without_restart() {
    dormant_cutover(false).await;
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn dormant_username_set_after_creation_is_checked_and_provisioned() {
    dormant_cutover(true).await;
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn dormant_unnamed_profile_refuses_before_p4_and_can_be_recovered() {
    let r = Rehearsal::new().await;
    let local = seed_history(&r, None, false).await;
    deactivate(&r, &local).await;
    r.seed_legacy_federation(&local).await;
    let before = r.retained_snapshot().await;
    let legacy = r.legacy_counts().await;
    let migrations: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations")
        .fetch_one(&r.iso.pool)
        .await
        .unwrap();
    let error = format!("{:?}", r.iso.try_apply_reset().await.unwrap_err());
    assert!(error.contains("1 profile(s) have no username"), "{error}");
    assert!(error.contains("deactivated/unprojected"), "{error}");
    assert_eq!(r.retained_snapshot().await, before);
    assert_eq!(r.legacy_counts().await, legacy);
    let applied: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations")
        .fetch_one(&r.iso.pool)
        .await
        .unwrap();
    assert_eq!(
        applied, migrations,
        "not even the first P4 migration was applied"
    );
    let ledger: Option<String> =
        sqlx::query_scalar("SELECT to_regclass('profile_federation_resets')::text")
            .fetch_one(&r.iso.pool)
            .await
            .unwrap();
    assert!(ledger.is_none(), "no reset ledger on refused cutover");

    // Recovery on the pre-P4 schema using the current full-history projector: reactivate,
    // assign username, and optionally disable again. This does NOT claim an old binary can
    // restore old streams outside its tailing window (see the runbook stop boundary).
    reactivate(&r, &local).await;
    let mut conn = r.module.database_connection().connection().await.unwrap();
    r.module
        .profile_repository()
        .save(
            &mut conn,
            Profile::set_username(
                local.profile.id().clone(),
                ProfileUsername::new("recovered-profile"),
            ),
        )
        .await
        .unwrap();
    drop(conn);
    project(&r).await;
    deactivate(&r, &local).await;
    r.iso.apply_reset().await;
    assert_eq!(provision_profile_keys(&r.module).await.unwrap().created, 1);
    reactivate(&r, &local).await;
    let (status, actor) = r
        .get_json(&format!("/ap/profiles/{}", local.profile.nanoid().as_ref()))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(actor["preferredUsername"], "recovered-profile");
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn older_precheck_can_be_added_to_an_already_cut_over_database_without_resetting_it() {
    let r = Rehearsal::new().await;
    // Reproduce an original P4 deployment: every migration except the new earlier precheck.
    let full = sqlx::migrate!("../migrations");
    let original_p4 = sqlx::migrate::Migrator {
        migrations: std::borrow::Cow::Owned(
            full.migrations
                .iter()
                .filter(|m| m.version != PRECHECK_VERSION)
                .cloned()
                .collect(),
        ),
        ignore_missing: false,
        locking: true,
    };
    original_p4.run(&r.iso.pool).await.unwrap();
    let local = seed_history(&r, None, false).await;
    deactivate(&r, &local).await;
    r.seed_legacy_federation(&local).await;
    let before = r.retained_snapshot().await;
    let legacy = r.legacy_counts().await;
    // The added precheck is cutover-only; the ledger prevents a spurious restart refusal.
    r.iso.apply_reset().await;
    assert_eq!(r.retained_snapshot().await, before);
    assert_eq!(r.legacy_counts().await, legacy, "no repeated reset");
    let applied: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM _sqlx_migrations WHERE version = $1 AND success)",
    )
    .bind(PRECHECK_VERSION)
    .fetch_one(&r.iso.pool)
    .await
    .unwrap();
    assert!(
        applied,
        "sqlx must apply the previously absent earlier version"
    );
    assert_eq!(
        provision_profile_keys(&r.module).await.unwrap().created,
        1,
        "an old deployment's missing dormant key is also repaired on restart"
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_passed_versioned_precheck_does_not_hide_new_dormant_profiles_on_retry() {
    let r = Rehearsal::new().await;
    let full = sqlx::migrate!("../migrations");
    let through_precheck = sqlx::migrate::Migrator {
        migrations: std::borrow::Cow::Owned(
            full.migrations
                .iter()
                .filter(|m| m.version <= PRECHECK_VERSION)
                .cloned()
                .collect(),
        ),
        ignore_missing: false,
        locking: true,
    };
    through_precheck.run(&r.iso.pool).await.unwrap();
    // Reproduce recovery after a later migration failed: the precheck version is already
    // recorded, but the old binary can now have created and deactivated another unnamed Profile.
    let local = seed_history(&r, None, false).await;
    deactivate(&r, &local).await;
    let before = r.retained_snapshot().await;
    let error = format!("{:?}", r.iso.try_apply_reset().await.unwrap_err());
    assert!(error.contains("1 profile(s) have no username"), "{error}");
    assert_eq!(r.retained_snapshot().await, before);
    let applied: i64 =
        sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations WHERE version > $1")
            .bind(PRECHECK_VERSION)
            .fetch_one(&r.iso.pool)
            .await
            .unwrap();
    assert_eq!(
        applied, 0,
        "retry must refuse before applying the remaining P4 migrations"
    );
}
