//! Cutover rehearsal on a real database (ADR profile-federation-in-place-reset).
//!
//! A database in the state right before the cutover (all earlier migrations applied, legacy
//! Account federation data present, Profiles with usernames but no actor keys) is switched with
//! the very steps the new binary performs on start: the remaining migrations (the destructive
//! reset), then the one-time Profile key provisioning. Then the Profile federation is checked
//! through the production router and the delivery worker.

use crate::api::profile_delivery_worker::spawn_profile_delivery_worker;
use crate::cutover::provision_profile_keys;
use crate::handler::AppModule;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use driver::crypto::FilePasswordProvider;
use driver::database::isolated_db::IsolatedDatabase;
use driver::http_signing::HttpSignatureVerifierImpl;
use http_body_util::BodyExt;
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::http_signing::{
    HttpSignatureVerificationInput, HttpSignatureVerifier, SignatureVerificationResult,
};
use kernel::interfaces::read_model::{
    AccountReadModel, DependOnAccountReadModel, DependOnProfileReadModel, ProfileReadModel,
};
use kernel::interfaces::repository::{
    AuthAccountRepository, AuthHostRepository, DependOnAuthAccountRepository,
    DependOnAuthHostRepository,
};
use kernel::prelude::entity::{
    Account, AccountKind, AuthAccountId, AuthHostId, Nanoid, Profile, ProfileDeliveryPolicy,
    ProfileId,
};
use kernel::test_utils::{
    unique_account_name, AccountBuilder, AuthAccountBuilder, AuthHostBuilder, ProfileBuilder,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[path = "cutover_dormant_tests.rs"]
mod dormant;

const RESET_TARGETS: [&str; 6] = [
    "follows",
    "blocks",
    "mutes",
    "remote_accounts",
    "outbox_activities",
    "signing_keys",
];

type Snapshot = BTreeMap<String, (i64, String)>;

/// `apply_reset` applies every remaining production migration, so a migration numbered after the
/// reset (new tables of a later feature) may add tables; the retained check covers the tables that
/// existed before the cutover.
fn only_tables_of(snapshot: Snapshot, reference: &Snapshot) -> Snapshot {
    snapshot
        .into_iter()
        .filter(|(table, _)| reference.contains_key(table))
        .collect()
}

struct Rehearsal {
    iso: IsolatedDatabase,
    module: AppModule,
    peer: MockServer,
    host_id: AuthHostId,
    _password: tempfile::NamedTempFile,
}

struct Local {
    auth_account_id: AuthAccountId,
    account: Account,
    profile: Profile,
}

impl Rehearsal {
    async fn new() -> Self {
        kernel::ensure_generator_initialized();
        let iso = IsolatedDatabase::create_before_cutover().await;
        let password = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(password.path(), b"cutover-rehearsal-password").unwrap();
        let peer = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/relation-tuples/check"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"allowed": true})))
            .mount(&peer)
            .await;
        for verb in ["PUT", "DELETE"] {
            Mock::given(method(verb))
                .and(path("/admin/relation-tuples"))
                .respond_with(ResponseTemplate::new(204))
                .mount(&peer)
                .await;
        }
        let module = AppModule::new_for_test_database(
            iso.database.clone(),
            "http://localhost:65535".to_string(),
            "http://localhost:65535".to_string(),
            peer.uri(),
            peer.uri(),
        )
        .await
        .expect("AppModule init failed (is DATABASE_URL set?)")
        .with_database(iso.database.clone())
        .with_loopback_profile_delivery()
        .with_password_provider(FilePasswordProvider::with_paths(
            password.path(),
            password.path(),
        ));
        let host_id = AuthHostId::default();
        let mut executor = module.database_connection().connection().await.unwrap();
        module
            .auth_host_repository()
            .create(
                &mut executor,
                &AuthHostBuilder::new()
                    .id(host_id.clone())
                    .url(format!("https://issuer.example/{}", uuid::Uuid::new_v4()))
                    .build(),
            )
            .await
            .unwrap();
        drop(executor);
        Self {
            iso,
            module,
            peer,
            host_id,
            _password: password,
        }
    }

    /// A pre-cutover Profile: it has a username, but no actor key.
    async fn seed_local(&self, username: &str) -> Local {
        let auth_account_id = AuthAccountId::default();
        let account = AccountBuilder::new()
            .kind(AccountKind::Personal)
            .name(unique_account_name())
            .build();
        let profile = ProfileBuilder::new()
            .id(ProfileId::new(kernel::generate_id()))
            .account_id(account.id().clone())
            .nanoid(Nanoid::default())
            .username(username)
            .build();
        let mut executor = self
            .module
            .database_connection()
            .connection()
            .await
            .unwrap();
        self.module
            .account_read_model()
            .create(&mut executor, &account)
            .await
            .unwrap();
        self.module
            .auth_account_repository()
            .create(
                &mut executor,
                &AuthAccountBuilder::new()
                    .id(auth_account_id.clone())
                    .host(self.host_id.clone())
                    .client_id(format!("subject-{}", uuid::Uuid::new_v4()))
                    .build(),
            )
            .await
            .unwrap();
        self.module
            .account_read_model()
            .link_auth_account(&mut executor, account.id(), &auth_account_id)
            .await
            .unwrap();
        self.module
            .profile_read_model()
            .create(&mut executor, &profile)
            .await
            .unwrap();
        Local {
            auth_account_id,
            account,
            profile,
        }
    }

    /// Legacy Account federation data in every reset target (raw rows, as the old binary wrote).
    async fn seed_legacy_federation(&self, local: &Local) {
        let pool = &self.iso.pool;
        let account_id = *local.account.id().as_ref();
        let remote_id = kernel::generate_id();
        sqlx::query(
            "INSERT INTO remote_accounts (id, acct, url, inbox_url, public_key_pem) \
             VALUES ($1, $2, $3, 'https://r.example/inbox', 'pem')",
        )
        .bind(remote_id)
        .bind(format!("bob{remote_id}@r.example"))
        .bind(format!("https://r.example/users/{remote_id}"))
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
        sqlx::query(
            "INSERT INTO blocks (id, blocker_local_id, blocked_remote_id) VALUES ($1, $2, $3)",
        )
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
            "https://old.example/activities/{}",
            kernel::generate_id()
        ))
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO signing_keys (id, account_id, algorithm, encrypted_private_key, public_key_pem, key_id_uri) \
             VALUES ($1, $2, 'rsa', '{}'::jsonb, 'old-pem', 'https://old.example/key')",
        )
        .bind(kernel::generate_id())
        .bind(account_id)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn count(&self, table: &str) -> i64 {
        sqlx::query_scalar(&format!("SELECT count(*)::bigint FROM \"{table}\""))
            .fetch_one(&self.iso.pool)
            .await
            .unwrap()
    }

    async fn legacy_counts(&self) -> Vec<i64> {
        let mut counts = Vec::new();
        for table in RESET_TARGETS {
            counts.push(self.count(table).await);
        }
        counts
    }

    /// (row count, content hash) of every table the reset must not touch, the key table included.
    async fn retained_snapshot(&self) -> Snapshot {
        let tables: Vec<String> = sqlx::query_scalar(
            "SELECT tablename FROM pg_tables WHERE schemaname = 'public' ORDER BY 1",
        )
        .fetch_all(&self.iso.pool)
        .await
        .unwrap();
        let mut snapshot = Snapshot::new();
        for table in tables {
            if RESET_TARGETS.contains(&table.as_str())
                || table == "profile_federation_resets"
                || table == "_sqlx_migrations"
                || table == "profile_signing_keys"
            {
                continue;
            }
            let (count, digest): (i64, String) = sqlx::query_as(&format!(
                "SELECT count(*)::bigint, coalesce(md5(string_agg(t::text, '|' ORDER BY t::text)), '') \
                 FROM \"{table}\" t"
            ))
            .fetch_one(&self.iso.pool)
            .await
            .unwrap();
            snapshot.insert(table, (count, digest));
        }
        snapshot
    }

    async fn key_rows(&self) -> Vec<(i64, i64, String, String)> {
        sqlx::query_as(
            "SELECT id, profile_id, encrypted_private_key::text, public_key_pem \
             FROM profile_signing_keys ORDER BY profile_id",
        )
        .fetch_all(&self.iso.pool)
        .await
        .unwrap()
    }

    async fn get_json(&self, uri: &str) -> (StatusCode, Value) {
        let router = crate::route::build_test_router(self.module.clone());
        let response = router
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(uri)
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
}

fn actor_url(module: &AppModule, profile: &Profile) -> String {
    use kernel::interfaces::config::DependOnPublicBaseUrl;
    format!(
        "{}/ap/profiles/{}",
        module.public_base_url().as_str().trim_end_matches('/'),
        profile.nanoid().as_ref()
    )
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn cutover_rehearsal_resets_only_legacy_data_and_provisions_keys_once() {
    let r = Rehearsal::new().await;
    let alice = r.seed_local("alice").await;
    let bob = r.seed_local("bob.b_c-d").await;
    r.seed_legacy_federation(&alice).await;
    r.seed_legacy_federation(&bob).await;
    assert!(r.legacy_counts().await.iter().all(|count| *count > 0));
    assert_eq!(r.count("profile_signing_keys").await, 0);
    let before = r.retained_snapshot().await;
    for table in ["accounts", "profiles", "auth_accounts", "auth_hosts"] {
        assert!(before[table].0 > 0, "{table} must hold retained data");
    }

    // Step 1 (the new binary's start): the remaining migrations.
    r.iso.apply_reset().await;

    assert_eq!(r.legacy_counts().await, vec![0; RESET_TARGETS.len()]);
    assert_eq!(
        only_tables_of(r.retained_snapshot().await, &before),
        before,
        "no retained table may change"
    );
    assert_eq!(r.count("profile_federation_resets").await, 1);

    // Step 2: the one-time key provisioning creates a key for every Profile, nothing else.
    let report = provision_profile_keys(&r.module).await.unwrap();
    assert_eq!(report.created, 2);
    let keys = r.key_rows().await;
    assert_eq!(keys.len(), 2);
    assert_eq!(
        only_tables_of(r.retained_snapshot().await, &before),
        before,
        "provisioning only adds keys"
    );

    // A restart (or a second process) repeats both steps: nothing is erased or regenerated.
    r.seed_legacy_federation(&alice).await;
    let legacy_after_cutover = r.legacy_counts().await;
    r.iso.apply_reset().await;
    r.iso.rerun_reset_sql().await;
    let report = provision_profile_keys(&r.module).await.unwrap();
    assert_eq!(report.created, 0, "no key is generated twice");
    assert_eq!(r.key_rows().await, keys, "existing keys are never replaced");
    assert_eq!(
        r.legacy_counts().await,
        legacy_after_cutover,
        "data created after the reset survives a re-run"
    );
    assert_eq!(r.count("profile_federation_resets").await, 1);

    // A Profile that appears later without a key (for example a late projection) gets exactly one.
    let carol = r.seed_local("carol").await;
    let report = provision_profile_keys(&r.module).await.unwrap();
    assert_eq!(report.created, 1);
    let keys_after = r.key_rows().await;
    assert_eq!(keys_after.len(), 3);
    for existing in &keys {
        assert!(keys_after.contains(existing), "old keys are untouched");
    }
    let _ = carol;
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn migrated_profiles_are_actors_with_a_verifiable_key_and_deliver_a_signed_follow() {
    let r = Rehearsal::new().await;
    // Usernames with every character the format allows beyond letters and digits.
    let alice = r.seed_local("alice.b_c-d").await;
    r.iso.apply_reset().await;
    assert_eq!(provision_profile_keys(&r.module).await.unwrap().created, 1);

    // The actor document and WebFinger of the migrated Profile resolve, and the document
    // publishes exactly the stored public key.
    let url = actor_url(&r.module, &alice.profile);
    let (status, actor) = r
        .get_json(&format!("/ap/profiles/{}", alice.profile.nanoid().as_ref()))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(actor["id"], url.as_str());
    assert_eq!(actor["preferredUsername"], "alice.b_c-d");
    let key_rows = r.key_rows().await;
    let public_pem = key_rows[0].3.clone();
    assert_eq!(actor["publicKey"]["publicKeyPem"], public_pem.as_str());
    assert_eq!(actor["publicKey"]["id"], format!("{url}#main-key").as_str());
    let host = url::Url::parse(&url).unwrap();
    let host = match host.port() {
        Some(port) => format!("{}:{port}", host.host_str().unwrap()),
        None => host.host_str().unwrap().to_string(),
    };
    let (status, jrd) = r
        .get_json(&format!(
            "/.well-known/webfinger?resource=acct:alice.b_c-d@{host}"
        ))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(jrd["links"][0]["href"], url.as_str());

    // A Follow of a remote actor is signed with the Profile key the actor document publishes.
    let remote_actor = format!("{}/users/bob", r.peer.uri());
    Mock::given(method("GET"))
        .and(path("/users/bob"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": remote_actor,
            "type": "Person",
            "inbox": format!("{}/inbox", r.peer.uri()),
        })))
        .mount(&r.peer)
        .await;
    Mock::given(method("POST"))
        .and(path("/inbox"))
        .respond_with(ResponseTemplate::new(202))
        .mount(&r.peer)
        .await;
    {
        use application::service::profile_relationship::ProfileRelationshipUseCase;
        r.module
            .follow_profile(
                &alice.auth_account_id,
                None,
                alice.profile.nanoid().as_ref(),
                &remote_actor,
            )
            .await
            .unwrap();
    }
    let (shutdown, handle) = spawn_profile_delivery_worker(
        Arc::new(r.module.clone()),
        ProfileDeliveryPolicy::default(),
        Duration::from_millis(50),
    );
    let mut delivered = false;
    for _ in 0..200 {
        if r.count("profile_delivery_jobs").await == 1
            && sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM profile_delivery_jobs WHERE state = 'delivered'",
            )
            .fetch_one(&r.iso.pool)
            .await
            .unwrap()
                == 1
        {
            delivered = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    shutdown.trigger();
    tokio::time::timeout(Duration::from_secs(10), handle)
        .await
        .expect("the worker stops after shutdown")
        .unwrap();
    assert!(delivered, "the worker delivered the Follow");

    let inbox_requests: Vec<_> = r
        .peer
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| request.method.as_str() == "POST")
        .collect();
    assert_eq!(inbox_requests.len(), 1);
    let request = &inbox_requests[0];
    let key_id = format!("{url}#main-key");
    let mut verifier = HttpSignatureVerifierImpl::new().unwrap();
    verifier.cache_actor_key(&key_id, public_pem);
    let headers: HashMap<String, String> = request
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
            url: format!("{}/inbox", r.peer.uri()),
            headers,
            body: Some(request.body.clone()),
        })
        .await
        .unwrap();
    assert_eq!(
        verified,
        SignatureVerificationResult::Valid { key_id },
        "the signature verifies against the public key of the actor document"
    );
    let activity: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(activity["type"], "Follow");
    assert_eq!(activity["actor"], url.as_str());
    assert_eq!(activity["object"], remote_actor.as_str());
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn account_and_organization_profile_creation_generate_the_profile_key_only() {
    use application::dto::account::CreateAccountDto;
    use application::service::account::CreateAccountUseCase;
    use application::service::profile::CreateOrganizationProfileUseCase;

    kernel::ensure_generator_initialized();
    let iso = IsolatedDatabase::create().await;
    let password = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(password.path(), b"creation-key-password").unwrap();
    let keto = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/admin/relation-tuples"))
        .respond_with(ResponseTemplate::new(201))
        .mount(&keto)
        .await;
    let module = AppModule::new_for_test_database(
        iso.database.clone(),
        "http://localhost:65535".to_string(),
        "http://localhost:65535".to_string(),
        keto.uri(),
        keto.uri(),
    )
    .await
    .unwrap()
    .with_password_provider(FilePasswordProvider::with_paths(
        password.path(),
        password.path(),
    ));
    let host_id = AuthHostId::default();
    let auth_account_id = AuthAccountId::default();
    {
        let mut executor = module.database_connection().connection().await.unwrap();
        module
            .auth_host_repository()
            .create(
                &mut executor,
                &AuthHostBuilder::new()
                    .id(host_id.clone())
                    .url(format!("https://issuer.example/{}", uuid::Uuid::new_v4()))
                    .build(),
            )
            .await
            .unwrap();
        module
            .auth_account_repository()
            .create(
                &mut executor,
                &AuthAccountBuilder::new()
                    .id(auth_account_id.clone())
                    .host(host_id)
                    .client_id(format!("subject-{}", uuid::Uuid::new_v4()))
                    .build(),
            )
            .await
            .unwrap();
    }

    let name = unique_account_name().as_ref().to_string();
    module
        .create_account_with_profile_username(
            auth_account_id.clone(),
            CreateAccountDto {
                name: name.clone(),
                is_bot: false,
            },
            "creator.name_1-x".to_string(),
        )
        .await
        .unwrap();
    let (profile_id, nanoid): (i64, String) = sqlx::query_as(
        "SELECT p.id, p.nanoid FROM profiles p JOIN accounts a ON a.id = p.account_id WHERE a.name = $1",
    )
    .bind(&name)
    .fetch_one(&iso.pool)
    .await
    .unwrap();
    let keys: Vec<(String, Option<time::OffsetDateTime>)> = sqlx::query_as(
        "SELECT key_id_uri, revoked_at FROM profile_signing_keys WHERE profile_id = $1",
    )
    .bind(profile_id)
    .fetch_all(&iso.pool)
    .await
    .unwrap();
    assert_eq!(
        keys.len(),
        1,
        "the Profile has exactly one key from creation"
    );
    assert!(keys[0]
        .0
        .ends_with(&format!("/ap/profiles/{nanoid}#main-key")));
    assert!(keys[0].1.is_none());
    let legacy_keys: i64 = sqlx::query_scalar("SELECT count(*) FROM signing_keys")
        .fetch_one(&iso.pool)
        .await
        .unwrap();
    assert_eq!(
        legacy_keys, 0,
        "no Account level signing key is created any more"
    );

    // A failed creation (taken username) leaves neither an Account, a Profile nor a key.
    let duplicate_name = unique_account_name().as_ref().to_string();
    let failed = module
        .create_account_with_profile_username(
            auth_account_id,
            CreateAccountDto {
                name: duplicate_name.clone(),
                is_bot: false,
            },
            "creator.name_1-x".to_string(),
        )
        .await;
    assert!(failed.is_err());
    let keys_now: i64 = sqlx::query_scalar("SELECT count(*) FROM profile_signing_keys")
        .fetch_one(&iso.pool)
        .await
        .unwrap();
    assert_eq!(keys_now, 1, "a rolled back creation leaves no key");
    let leftover: i64 = sqlx::query_scalar("SELECT count(*) FROM accounts WHERE name = $1")
        .bind(&duplicate_name)
        .fetch_one(&iso.pool)
        .await
        .unwrap();
    assert_eq!(leftover, 0);

    // An organization Profile gets its key in the creation transaction as well.
    let organization = AccountBuilder::new()
        .kind(AccountKind::Organization)
        .name(unique_account_name())
        .build();
    {
        let mut executor = module.database_connection().connection().await.unwrap();
        module
            .account_read_model()
            .create(&mut executor, &organization)
            .await
            .unwrap();
    }
    let created = module
        .create_organization_profile(
            application::service::session_context::OrganizationContext {
                org_account_id: organization.id().clone(),
                org_account_nanoid: organization.nanoid().as_ref().to_string(),
                role: kernel::prelude::entity::OrgRole::Member,
            },
            application::dto::profile::CreateProfileDto {
                display_name: Some("Org".to_string()),
                summary: None,
                username: "org.profile_1-x".to_string(),
            },
        )
        .await
        .unwrap();
    let org_keys: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM profile_signing_keys k JOIN profiles p ON p.id = k.profile_id \
         WHERE p.nanoid = $1 AND k.revoked_at IS NULL",
    )
    .bind(&created.nanoid)
    .fetch_one(&iso.pool)
    .await
    .unwrap();
    assert_eq!(org_keys, 1);
}
