//! Real-database tests of the Profile identity across its lifecycle: a transfer racing an edit,
//! and deactivation followed by reactivation (the key and the actor must come back without a
//! restart).

use crate::api::profile_delivery_worker::spawn_profile_delivery_worker;
use crate::cutover::provision_profile_keys;
use crate::route::profile_graph_test_support::GraphWorld;
use application::dto::account::UpdateAccountDto;
use application::projection::{ProjectAccountBatch, ProjectProfileBatch};
use application::service::account::{DeactivateAccountUseCase, ReactivateAccountUseCase};
use application::service::account_detail::UpdateAccountDetailUseCase;
use application::service::profile_relationship::ProfileRelationshipUseCase;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use driver::http_signing::HttpSignatureVerifierImpl;
use http_body_util::BodyExt;
use kernel::interfaces::database::{
    DatabaseConnection, DependOnDatabaseConnection, DependOnTransactionManager, TransactionManager,
};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::http_signing::{
    HttpSignatureVerificationInput, HttpSignatureVerifier, SignatureVerificationResult,
};
use kernel::interfaces::read_model::{
    AccountQuery, AccountReadModel, DependOnAccountQuery, DependOnAccountReadModel,
    DependOnProfileReadModel, ProfileReadModel,
};
use kernel::interfaces::repository::{
    AggregateRepository, DependOnAccountRepository, DependOnProfileRepository,
};
use kernel::prelude::entity::{
    Account, AccountId, AccountIsBot, AccountKind, FieldAction, Nanoid, Profile,
    ProfileDeliveryPolicy, ProfileDisplayName,
};
use kernel::test_utils::{unique_account_name, AccountBuilder};
use kernel::KernelError;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn account_nanoid_of(world: &GraphWorld, profile_nanoid: &str) -> String {
    sqlx::query_scalar(
        "SELECT a.nanoid FROM accounts a JOIN profiles p ON p.account_id = a.id WHERE p.nanoid = $1",
    )
    .bind(profile_nanoid)
    .fetch_one(world.pool())
    .await
    .unwrap()
}

async fn profile_row(
    world: &GraphWorld,
    profile_nanoid: &str,
) -> Option<(i64, Option<String>, i64)> {
    sqlx::query_as("SELECT account_id, display, version FROM profiles WHERE nanoid = $1")
        .bind(profile_nanoid)
        .fetch_optional(world.pool())
        .await
        .unwrap()
}

async fn get_actor(world: &GraphWorld, nanoid: &str) -> (StatusCode, Value) {
    let response = world
        .root_router()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/ap/profiles/{nanoid}"))
                .header("accept", "application/activity+json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Seq of the newest event of a Profile stream and the checkpoint of the Profile projector.
async fn newest_seq_and_checkpoint(world: &GraphWorld, profile_id: i64) -> (i64, i64) {
    let newest: i64 = sqlx::query_scalar("SELECT max(seq) FROM profile_events WHERE id = $1")
        .bind(profile_id)
        .fetch_one(world.pool())
        .await
        .unwrap();
    let checkpoint: i64 = sqlx::query_scalar(
        "SELECT last_seq FROM projection_checkpoints WHERE projector_name = 'profile_projector'",
    )
    .fetch_one(world.pool())
    .await
    .unwrap();
    (newest, checkpoint)
}

/// Runs the tailing projectors (as the server's projection worker does) a few rounds.
async fn project(world: &GraphWorld) {
    for _ in 0..4 {
        world.module.project_batch().await.unwrap();
        world.module.project_profile_batch().await.unwrap();
    }
}

/// A transfer holds the Profile row while an edit arrives: the edit must wait, rebuild the
/// aggregate from all committed events, and write a row that still has the new owner (and the
/// projectors must not change that afterwards).
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn an_edit_racing_a_transfer_keeps_the_new_owner_and_the_edit() {
    let world = GraphWorld::new().await;
    let user = world.new_user().await;
    let profile = world.create_profile(&user, true).await;
    let account_nanoid = account_nanoid_of(&world, &profile.nanoid).await;
    let organization = AccountBuilder::new()
        .kind(AccountKind::Organization)
        .name(unique_account_name())
        .build();
    {
        let mut conn = world
            .module
            .database_connection()
            .connection()
            .await
            .unwrap();
        world
            .module
            .account_read_model()
            .create(&mut conn, &organization)
            .await
            .unwrap();
    }
    let personal = {
        let mut conn = world
            .module
            .database_connection()
            .connection()
            .await
            .unwrap();
        world
            .module
            .account_query()
            .find_by_nanoid(&mut conn, &Nanoid::<Account>::new(account_nanoid.clone()))
            .await
            .unwrap()
            .unwrap()
    };

    // The transfer: lock the Profile, append the event, write the row, and only commit later.
    let (locked_tx, locked_rx) = tokio::sync::oneshot::channel::<()>();
    let transfer = {
        let module = world.module.clone();
        let profile_id = profile.id.clone();
        let from = personal.id().clone();
        let to = organization.id().clone();
        tokio::spawn(async move {
            let deps = module.clone();
            let mut locked_tx = Some(locked_tx);
            module
                .transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        deps.profile_read_model()
                            .lock_by_id_for_update(executor, &profile_id)
                            .await?
                            .ok_or_else(|| error_stack::Report::new(KernelError::NotFound))?;
                        let (loaded, _) = deps
                            .profile_repository()
                            .load(executor, &profile_id)
                            .await?
                            .into_parts();
                        let event = deps
                            .profile_repository()
                            .save(
                                executor,
                                Profile::transfer_account(profile_id.clone(), from, to),
                            )
                            .await?;
                        let mut folded = Some(loaded);
                        Profile::apply(&mut folded, event)?;
                        deps.profile_read_model()
                            .update(executor, &folded.unwrap())
                            .await?;
                        if let Some(tx) = locked_tx.take() {
                            let _ = tx.send(());
                        }
                        tokio::time::sleep(Duration::from_millis(1200)).await;
                        Ok(())
                    })
                })
                .await
                .unwrap();
        })
    };
    locked_rx.await.unwrap();

    // The edit starts while the transfer is uncommitted: it sees the old owner in the projection.
    let started = Instant::now();
    world
        .module
        .update_account_detail_authorized(
            UpdateAccountDto {
                account_nanoid: account_nanoid.clone(),
                is_bot: FieldAction::Unchanged,
                display_name: FieldAction::Set("edited during the transfer".to_string()),
                summary: FieldAction::Unchanged,
                icon_url: FieldAction::Unchanged,
                banner_url: FieldAction::Unchanged,
                fields: None,
            },
            personal,
        )
        .await
        .unwrap();
    assert!(
        started.elapsed() >= Duration::from_millis(500),
        "the edit must wait for the transfer that holds the Profile row (waited {:?})",
        started.elapsed()
    );
    transfer.await.unwrap();

    let (owner, display, version) = profile_row(&world, &profile.nanoid).await.unwrap();
    assert_eq!(
        owner,
        *organization.id().as_ref(),
        "the edit must not bring the old owner back"
    );
    assert_eq!(display.as_deref(), Some("edited during the transfer"));
    let (events, newest): (i64, i64) =
        sqlx::query_as("SELECT count(*), max(version) FROM profile_events WHERE id = $1")
            .bind(profile.id.as_ref())
            .fetch_one(world.pool())
            .await
            .unwrap();
    assert_eq!(events, 3, "Created, AccountTransferred and Updated");
    assert_eq!(version, newest, "the row carries the newest event version");

    // The tailing projectors replay the event stream and leave the row as it is, and ownership
    // checks (which read this row) see the organization.
    project(&world).await;
    let (owner_after, display_after, version_after) =
        profile_row(&world, &profile.nanoid).await.unwrap();
    assert_eq!(
        (owner_after, display_after.clone(), version_after),
        (owner, display, version)
    );
    let nanoid = profile.nanoid.clone();
    let deps = world.module.clone();
    let seen_owner = world
        .module
        .transaction_manager()
        .transaction(move |executor| {
            Box::pin(async move {
                use kernel::interfaces::repository::DependOnDomainControlRepository;
                use kernel::interfaces::repository::DomainControlRepository;
                Ok(deps
                    .domain_control_repository()
                    .lock_profile_owner(executor, &nanoid)
                    .await?
                    .map(|(_, owner)| owner))
            })
        })
        .await
        .unwrap();
    assert_eq!(seen_owner.as_ref(), Some(organization.id()));
}

/// Deactivating and reactivating an Account takes the Profile row away and brings it back; the
/// Profile key must come back with it, so the actor works again without a restart and without a
/// new identity.
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn reactivation_brings_the_actor_back_with_the_same_key_and_without_a_restart() {
    let world = GraphWorld::new().await;
    world.allow_all_permissions().await;
    let user = world.new_user().await;
    let profile = world.create_profile(&user, true).await;
    let account_nanoid = account_nanoid_of(&world, &profile.nanoid).await;
    let keys_before: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT id, key_id_uri, public_key_pem FROM profile_signing_keys WHERE profile_id = $1",
    )
    .bind(profile.id.as_ref())
    .fetch_all(world.pool())
    .await
    .unwrap();
    assert_eq!(keys_before.len(), 1);
    let (status, actor_before) = get_actor(&world, &profile.nanoid).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        actor_before["publicKey"]["publicKeyPem"],
        keys_before[0].2.as_str()
    );

    world
        .module
        .deactivate_account(&user.auth_account_id, account_nanoid.clone())
        .await
        .unwrap();
    project(&world).await;
    assert!(
        profile_row(&world, &profile.nanoid).await.is_none(),
        "the account projector removes the Profile row on deactivation"
    );
    assert_eq!(
        get_actor(&world, &profile.nanoid).await.0,
        StatusCode::NOT_FOUND
    );
    let retained: i64 =
        sqlx::query_scalar("SELECT count(*) FROM profile_signing_keys WHERE profile_id = $1")
            .bind(profile.id.as_ref())
            .fetch_one(world.pool())
            .await
            .unwrap();
    assert_eq!(
        retained, 1,
        "the key is retained while the Account is deactivated"
    );

    world
        .module
        .reactivate_account(&user.auth_account_id, account_nanoid.clone())
        .await
        .unwrap();
    project(&world).await;
    assert!(
        profile_row(&world, &profile.nanoid).await.is_some(),
        "the profile projector re-materializes the row on reactivation"
    );

    // No restart and no provisioning: the same key is published again.
    let (status, actor_after) = get_actor(&world, &profile.nanoid).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the actor resolves again after reactivation"
    );
    assert_eq!(actor_after["publicKey"]["id"], keys_before[0].1.as_str());
    assert_eq!(
        actor_after["publicKey"]["publicKeyPem"],
        keys_before[0].2.as_str()
    );
    let keys_after: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT id, key_id_uri, public_key_pem FROM profile_signing_keys WHERE profile_id = $1",
    )
    .bind(profile.id.as_ref())
    .fetch_all(world.pool())
    .await
    .unwrap();
    assert_eq!(keys_after, keys_before, "no key was regenerated");
    assert_eq!(
        provision_profile_keys(&world.module).await.unwrap().created,
        0,
        "a restart would not generate anything either"
    );

    deliver_follow_and_verify(
        &world,
        &user,
        &profile,
        &keys_before[0].1,
        &keys_before[0].2,
    )
    .await;
}

/// The Profile signs a Follow (queued through the real use case, sent by the real delivery worker
/// to a fake peer) that verifies against the key the actor document publishes.
async fn deliver_follow_and_verify(
    world: &GraphWorld,
    user: &crate::route::profile_graph_test_support::User,
    profile: &crate::route::profile_graph_test_support::LocalProfile,
    key_id: &str,
    public_key_pem: &str,
) {
    // And the reactivated Profile signs a Follow that verifies against the published key.
    let peer = MockServer::start().await;
    let remote_actor = format!("{}/users/bob", peer.uri());
    Mock::given(method("GET"))
        .and(path("/users/bob"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": remote_actor,
            "type": "Person",
            "inbox": format!("{}/inbox", peer.uri()),
        })))
        .mount(&peer)
        .await;
    Mock::given(method("POST"))
        .and(path("/inbox"))
        .respond_with(ResponseTemplate::new(202))
        .mount(&peer)
        .await;
    let module = world.module.clone().with_loopback_profile_delivery();
    module
        .follow_profile(&user.auth_account_id, None, &profile.nanoid, &remote_actor)
        .await
        .unwrap();
    let (shutdown, handle) = spawn_profile_delivery_worker(
        Arc::new(module),
        ProfileDeliveryPolicy::default(),
        Duration::from_millis(50),
    );
    let mut delivered = false;
    for _ in 0..200 {
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM profile_delivery_jobs WHERE state = 'delivered'",
        )
        .fetch_one(world.pool())
        .await
        .unwrap();
        if count == 1 {
            delivered = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    shutdown.trigger();
    tokio::time::timeout(Duration::from_secs(10), handle)
        .await
        .expect("the worker stops")
        .unwrap();
    assert!(delivered, "the reactivated Profile delivers its Follow");
    let requests: Vec<_> = peer
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| request.method.as_str() == "POST")
        .collect();
    assert_eq!(requests.len(), 1);
    let mut verifier = HttpSignatureVerifierImpl::new().unwrap();
    verifier.cache_actor_key(key_id, public_key_pem.to_string());
    let headers: HashMap<String, String> = requests[0]
        .headers
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_ascii_lowercase(),
                value.to_str().unwrap().to_string(),
            )
        })
        .collect();
    let verified = verifier
        .verify(&HttpSignatureVerificationInput {
            method: "POST".to_string(),
            url: format!("{}/inbox", peer.uri()),
            headers,
            body: Some(requests[0].body.clone()),
        })
        .await
        .unwrap();
    assert_eq!(
        verified,
        SignatureVerificationResult::Valid {
            key_id: key_id.to_string()
        }
    );
}

/// The Profile projector only looks at a window (100) of recent events. A Profile whose events
/// are far older than that window must still come back when its Account is reactivated, with the
/// same actor URL, key and a working signed Follow.
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn an_old_profile_is_restored_on_reactivation_whatever_the_tailing_window() {
    let world = GraphWorld::new().await;
    world.allow_all_permissions().await;
    let user = world.new_user().await;
    let profile = world.create_profile(&user, true).await;
    let account_nanoid = account_nanoid_of(&world, &profile.nanoid).await;
    let keys: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT id, key_id_uri, public_key_pem FROM profile_signing_keys WHERE profile_id = $1",
    )
    .bind(profile.id.as_ref())
    .fetch_all(world.pool())
    .await
    .unwrap();
    assert_eq!(keys.len(), 1);

    // Other Profiles make far more than a window of newer events, and the projector moves on.
    let noisy_user = world.new_user().await;
    let noisy = world.create_profile(&noisy_user, true).await;
    {
        let deps = world.module.clone();
        let noisy_id = noisy.id.clone();
        world
            .module
            .transaction_manager()
            .transaction(move |executor| {
                Box::pin(async move {
                    for n in 0..150 {
                        deps.profile_repository()
                            .save(
                                executor,
                                Profile::update(
                                    noisy_id.clone(),
                                    FieldAction::Set(ProfileDisplayName::new(format!("n{n}"))),
                                    FieldAction::Unchanged,
                                    FieldAction::Unchanged,
                                    FieldAction::Unchanged,
                                ),
                            )
                            .await?;
                    }
                    Ok(())
                })
            })
            .await
            .unwrap();
    }
    // Account events of another Account do the same to the Account projector's window.
    {
        let deps = world.module.clone();
        let noisy_account = AccountId::new(
            sqlx::query_scalar::<_, i64>("SELECT account_id FROM profiles WHERE id = $1")
                .bind(noisy.id.as_ref())
                .fetch_one(world.pool())
                .await
                .unwrap(),
        );
        world
            .module
            .transaction_manager()
            .transaction(move |executor| {
                Box::pin(async move {
                    for n in 0..150 {
                        let (_, version) = deps
                            .account_repository()
                            .load(executor, &noisy_account)
                            .await?
                            .into_parts();
                        deps.account_repository()
                            .save(
                                executor,
                                Account::update(
                                    noisy_account.clone(),
                                    AccountIsBot::new(n % 2 == 0),
                                    version,
                                ),
                            )
                            .await?;
                    }
                    Ok(())
                })
            })
            .await
            .unwrap();
    }
    project(&world).await;
    let account_checkpoint: i64 = sqlx::query_scalar(
        "SELECT last_seq FROM projection_checkpoints WHERE projector_name = 'account_projector'",
    )
    .fetch_one(world.pool())
    .await
    .unwrap();
    let account_newest: i64 = sqlx::query_scalar(
        "SELECT max(e.seq) FROM account_events e JOIN accounts a ON a.id = e.id WHERE a.nanoid = $1",
    )
    .bind(&account_nanoid)
    .fetch_one(world.pool())
    .await
    .unwrap();
    assert!(
        account_checkpoint - 100 > account_newest,
        "the Account's events must be outside the Account projector's window too"
    );
    let (newest, checkpoint) = newest_seq_and_checkpoint(&world, *profile.id.as_ref()).await;
    assert!(
        checkpoint - 100 > newest,
        "the Profile's events must be outside the tailing window (newest {newest}, checkpoint {checkpoint})"
    );
    // The edit that reaches the row is read model state; an old Profile with an edit history of
    // its own (a username-less stream would not matter here) keeps its username through all this.
    let username_before: Option<String> =
        sqlx::query_scalar("SELECT username FROM profiles WHERE id = $1")
            .bind(profile.id.as_ref())
            .fetch_one(world.pool())
            .await
            .unwrap();
    assert!(username_before.is_some());

    world
        .module
        .deactivate_account(&user.auth_account_id, account_nanoid.clone())
        .await
        .unwrap();
    project(&world).await;
    assert!(profile_row(&world, &profile.nanoid).await.is_none());
    assert_eq!(
        get_actor(&world, &profile.nanoid).await.0,
        StatusCode::NOT_FOUND
    );

    world
        .module
        .reactivate_account(&user.auth_account_id, account_nanoid.clone())
        .await
        .unwrap();
    project(&world).await;
    assert!(
        profile_row(&world, &profile.nanoid).await.is_some(),
        "the Profile must be restored although its events are outside the tailing window"
    );
    let username_after: Option<String> =
        sqlx::query_scalar("SELECT username FROM profiles WHERE id = $1")
            .bind(profile.id.as_ref())
            .fetch_one(world.pool())
            .await
            .unwrap();
    assert_eq!(username_after, username_before, "the identity is the same");
    let (status, actor) = get_actor(&world, &profile.nanoid).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(actor["id"], profile.actor_url.as_str());
    assert_eq!(actor["publicKey"]["id"], keys[0].1.as_str());
    assert_eq!(actor["publicKey"]["publicKeyPem"], keys[0].2.as_str());
    // The other Profile was never touched.
    assert!(profile_row(&world, &noisy.nanoid).await.is_some());

    deliver_follow_and_verify(&world, &user, &profile, &keys[0].1, &keys[0].2).await;
}
