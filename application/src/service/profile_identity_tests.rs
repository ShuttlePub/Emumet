//! P1 acceptance tests against a real database: username storage rules, the one-time setter,
//! Profile transfer invariants, Profile keys and the actor / WebFinger documents.

use super::TestModule;
use crate::dto::account::CreateAccountDto;
use crate::dto::activitypub::{GetProfileActorDto, GetProfileWebFingerDto};
use crate::profile_signing_key::{
    CreateProfileSigningKeyUseCase, GetProfilePublicKeyUseCase, SignProfileRequestUseCase,
};
use crate::service::account::{CreateAccountUseCase, DeactivateAccountUseCase};
use crate::service::activitypub::{GetProfileActorUseCase, GetProfileWebFingerUseCase};
use crate::service::profile_username::SetProfileUsernameUseCase;
use crate::service::session_context::OrganizationContext;
use kernel::interfaces::crypto::SigningAlgorithm;
use kernel::interfaces::database::{
    DatabaseConnection, DependOnTransactionManager, Transaction, TransactionManager,
    TransactionalDatabaseConnection,
};
use kernel::interfaces::http_signing::HttpSigningRequest;
use kernel::interfaces::read_model::{
    AccountReadModel, DependOnAccountReadModel, DependOnProfileReadModel, ProfileReadModel,
};
use kernel::interfaces::repository::{
    AggregateRepository, DependOnAccountRepository, DependOnProfileRepository,
    DependOnProfileSigningKeyRepository, ProfileSigningKeyRepository,
};
use kernel::prelude::entity::{
    Account, AccountId, AccountIsBot, AccountName, AuthAccountId, Nanoid, OrgRole, Profile,
    ProfileId,
};
use kernel::KernelError;
use std::collections::HashMap;

struct Env {
    module: TestModule,
    auth_account_id: AuthAccountId,
    _password_file: tempfile::NamedTempFile,
}

async fn env() -> Env {
    kernel::ensure_generator_initialized();
    let password_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(password_file.path(), b"profile-identity-password").unwrap();
    let module = TestModule::new(password_file.path()).await;
    let auth_account_id = AuthAccountId::default();
    module.seed_auth_account(&auth_account_id).await;
    Env {
        module,
        auth_account_id,
        _password_file: password_file,
    }
}

fn unique_username() -> String {
    format!("u{}", kernel::generate_id())
}

struct Created {
    account_nanoid: String,
    profile_id: ProfileId,
    profile_nanoid: Nanoid<Profile>,
}

async fn create_account(
    env: &Env,
    username: Option<String>,
) -> error_stack::Result<Created, KernelError> {
    let name = kernel::test_utils::unique_account_name()
        .as_ref()
        .to_string();
    let account = env
        .module
        .create_account_with_profile_username(
            env.auth_account_id.clone(),
            CreateAccountDto {
                name: name.clone(),
                is_bot: false,
            },
            username,
        )
        .await?;
    let (profile_id, nanoid): (i64, String) = sqlx::query_as(
        "SELECT p.id, p.nanoid FROM profiles p JOIN accounts a ON a.id = p.account_id WHERE a.name = $1",
    )
    .bind(&name)
    .fetch_one(&mut *env.module.database.connection().await.unwrap())
    .await
    .unwrap();
    Ok(Created {
        account_nanoid: account.nanoid,
        profile_id: ProfileId::new(profile_id),
        profile_nanoid: Nanoid::new(nanoid),
    })
}

async fn stored_username(env: &Env, profile_id: &ProfileId) -> Option<String> {
    sqlx::query_scalar::<_, Option<String>>("SELECT username FROM profiles WHERE id = $1")
        .bind(profile_id.as_ref())
        .fetch_optional(&mut *env.module.database.connection().await.unwrap())
        .await
        .unwrap()
        .flatten()
}

async fn count(env: &Env, sql: &str, bind: i64) -> i64 {
    sqlx::query_scalar::<_, i64>(sql)
        .bind(bind)
        .fetch_one(&mut *env.module.database.connection().await.unwrap())
        .await
        .unwrap()
}

fn context(error: &error_stack::Report<KernelError>) -> &KernelError {
    error.current_context()
}

async fn create_profile_key(
    env: &Env,
    created: &Created,
) -> error_stack::Result<kernel::prelude::entity::ProfileSigningKey, KernelError> {
    let deps = env.module.clone();
    let profile_id = created.profile_id.clone();
    let nanoid = created.profile_nanoid.clone();
    env.module
        .transaction_manager()
        .transaction(move |executor| {
            Box::pin(async move {
                deps.create_profile_signing_key(
                    executor,
                    profile_id,
                    &nanoid,
                    SigningAlgorithm::Rsa2048,
                )
                .await
            })
        })
        .await
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn account_creation_stores_the_username_and_legacy_creation_leaves_it_unset() {
    let env = env().await;
    let username = unique_username();
    let with = create_account(&env, Some(username.clone())).await.unwrap();
    let without = create_account(&env, None).await.unwrap();

    assert_eq!(
        stored_username(&env, &with.profile_id).await.as_deref(),
        Some(username.as_str())
    );
    assert_eq!(stored_username(&env, &without.profile_id).await, None);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn invalid_or_taken_usernames_are_rejected_without_creating_anything() {
    let env = env().await;
    let username = unique_username();
    create_account(&env, Some(username.clone())).await.unwrap();
    for invalid in ["Alice", "admin", " x", ""] {
        let err = create_account(&env, Some(invalid.to_string()))
            .await
            .map(|_| ())
            .unwrap_err();
        assert_eq!(context(&err), &KernelError::Validation, "{invalid:?}");
    }
    let err = create_account(&env, Some(username.clone()))
        .await
        .map(|_| ())
        .unwrap_err();
    assert_eq!(context(&err), &KernelError::Concurrency);

    let events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM profile_events WHERE data ->> 'username' = $1")
            .bind(&username)
            .fetch_one(&mut *env.module.database.connection().await.unwrap())
            .await
            .unwrap();
    assert_eq!(events, 1, "rejected attempts must not leave events behind");
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn owner_sets_the_username_exactly_once() {
    let env = env().await;
    let created = create_account(&env, None).await.unwrap();
    let nanoid = created.profile_nanoid.as_ref().to_string();
    let username = unique_username();

    let dto = env
        .module
        .set_profile_username(&env.auth_account_id, None, nanoid.clone(), username.clone())
        .await
        .unwrap();
    assert_eq!(dto.username.as_deref(), Some(username.as_str()));
    assert_eq!(dto.nanoid, nanoid);
    assert_eq!(
        stored_username(&env, &created.profile_id).await.as_deref(),
        Some(username.as_str())
    );

    for again in [username.clone(), unique_username()] {
        let err = env
            .module
            .set_profile_username(&env.auth_account_id, None, nanoid.clone(), again)
            .await
            .unwrap_err();
        assert_eq!(context(&err), &KernelError::Concurrency);
    }
    assert_eq!(
        stored_username(&env, &created.profile_id).await.as_deref(),
        Some(username.as_str()),
        "the username never changes once set"
    );

    let err = env
        .module
        .set_profile_username(
            &env.auth_account_id,
            None,
            nanoid.clone(),
            "Bad Name".into(),
        )
        .await
        .unwrap_err();
    assert_eq!(context(&err), &KernelError::Validation);
    let err = env
        .module
        .set_profile_username(
            &env.auth_account_id,
            None,
            "no-such-profile".into(),
            unique_username(),
        )
        .await
        .unwrap_err();
    assert_eq!(context(&err), &KernelError::NotFound);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn only_the_owning_context_may_set_the_username() {
    let env = env().await;
    let created = create_account(&env, None).await.unwrap();
    let foreign_organization = OrganizationContext {
        org_account_id: AccountId::default(),
        org_account_nanoid: "foreign-org".to_string(),
        role: OrgRole::Admin,
    };
    let err = env
        .module
        .set_profile_username(
            &env.auth_account_id,
            Some(&foreign_organization),
            created.profile_nanoid.as_ref().to_string(),
            unique_username(),
        )
        .await
        .unwrap_err();
    assert_eq!(context(&err), &KernelError::PermissionDenied);
    assert_eq!(stored_username(&env, &created.profile_id).await, None);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn concurrent_assignments_let_exactly_one_win() {
    let env = env().await;

    // One Profile, many different usernames: the Profile ends up with exactly one.
    let created = create_account(&env, None).await.unwrap();
    let mut handles = Vec::new();
    for _ in 0..8 {
        let module = env.module.clone();
        let auth = env.auth_account_id.clone();
        let nanoid = created.profile_nanoid.as_ref().to_string();
        let username = unique_username();
        handles.push(tokio::spawn(async move {
            module
                .set_profile_username(&auth, None, nanoid, username)
                .await
                .map(|dto| dto.username)
        }));
    }
    let mut winners = Vec::new();
    for handle in handles {
        match handle.await.unwrap() {
            Ok(username) => winners.push(username.unwrap()),
            Err(error) => assert_eq!(context(&error), &KernelError::Concurrency),
        }
    }
    assert_eq!(winners.len(), 1, "exactly one concurrent assignment wins");
    assert_eq!(
        stored_username(&env, &created.profile_id).await.as_deref(),
        Some(winners[0].as_str())
    );
    assert_eq!(
        count(
            &env,
            "SELECT count(*) FROM profile_events WHERE id = $1 AND data ->> 'username' IS NOT NULL",
            *created.profile_id.as_ref(),
        )
        .await,
        1
    );

    // Many Profiles, one username: exactly one Profile gets it.
    let shared = unique_username();
    let mut profiles = Vec::new();
    for _ in 0..6 {
        profiles.push(create_account(&env, None).await.unwrap());
    }
    let mut handles = Vec::new();
    for profile in &profiles {
        let module = env.module.clone();
        let auth = env.auth_account_id.clone();
        let nanoid = profile.profile_nanoid.as_ref().to_string();
        let username = shared.clone();
        handles.push(tokio::spawn(async move {
            module
                .set_profile_username(&auth, None, nanoid, username)
                .await
                .map(|_| ())
        }));
    }
    let mut ok = 0;
    for handle in handles {
        match handle.await.unwrap() {
            Ok(()) => ok += 1,
            Err(error) => assert_eq!(context(&error), &KernelError::Concurrency),
        }
    }
    assert_eq!(ok, 1);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_deleted_profiles_username_cannot_be_reused() {
    let env = env().await;
    let username = unique_username();
    let created = create_account(&env, Some(username.clone())).await.unwrap();

    env.module
        .deactivate_account(&env.auth_account_id, created.account_nanoid.clone())
        .await
        .unwrap();
    // The projection row of a deleted Profile disappears (Account projector cascade); do the same
    // explicitly in case the projector has not run.
    {
        let mut conn = env.module.database.connection().await.unwrap();
        env.module
            .profile_read_model()
            .delete(&mut conn, &created.profile_id)
            .await
            .unwrap();
    }
    assert_eq!(stored_username(&env, &created.profile_id).await, None);

    let err = create_account(&env, Some(username.clone()))
        .await
        .map(|_| ())
        .unwrap_err();
    assert_eq!(context(&err), &KernelError::Concurrency);

    let other = create_account(&env, None).await.unwrap();
    let err = env
        .module
        .set_profile_username(
            &env.auth_account_id,
            None,
            other.profile_nanoid.as_ref().to_string(),
            username,
        )
        .await
        .unwrap_err();
    assert_eq!(context(&err), &KernelError::Concurrency);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn production_paths_never_generate_profile_keys() {
    let env = env().await;
    let created = create_account(&env, Some(unique_username())).await.unwrap();
    let other = create_account(&env, None).await.unwrap();
    env.module
        .set_profile_username(
            &env.auth_account_id,
            None,
            other.profile_nanoid.as_ref().to_string(),
            unique_username(),
        )
        .await
        .unwrap();
    for profile in [&created, &other] {
        assert_eq!(
            count(
                &env,
                "SELECT count(*) FROM profile_signing_keys WHERE profile_id = $1",
                *profile.profile_id.as_ref(),
            )
            .await,
            0,
            "creating an Account/Profile or setting a username must not create Profile keys"
        );
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn profile_key_signs_and_exposes_its_public_key() {
    let env = env().await;
    let created = create_account(&env, Some(unique_username())).await.unwrap();
    let key = create_profile_key(&env, &created).await.unwrap();

    let expected_key_id = format!(
        "https://example.com/ap/profiles/{}#main-key",
        created.profile_nanoid.as_ref()
    );
    assert_eq!(key.key_id_uri, expected_key_id);
    assert_eq!(key.profile_id(), &created.profile_id);

    // A second active key for the same Profile is refused (no silent identity replacement).
    let err = create_profile_key(&env, &created).await.unwrap_err();
    assert_eq!(context(&err), &KernelError::Concurrency);

    let info = env
        .module
        .get_profile_public_key_info(&created.profile_id, &created.profile_nanoid)
        .await
        .unwrap();
    assert_eq!(info.id, expected_key_id);
    assert_eq!(
        info.owner,
        format!(
            "https://example.com/ap/profiles/{}",
            created.profile_nanoid.as_ref()
        )
    );
    assert_eq!(info.public_key_pem, key.public_key_pem);

    let mut headers = HashMap::new();
    headers.insert("host".to_string(), "remote.example".to_string());
    headers.insert(
        "date".to_string(),
        "Thu, 01 Jan 2026 00:00:00 GMT".to_string(),
    );
    headers.insert("digest".to_string(), "SHA-256=AAAA".to_string());
    let signed = env
        .module
        .sign_as_profile(
            &created.profile_id,
            HttpSigningRequest {
                method: "POST".to_string(),
                url: "https://remote.example/inbox".to_string(),
                headers,
                body: Some(b"{}".to_vec()),
            },
        )
        .await
        .unwrap();
    let signature = signed.cavage_headers.get("signature").unwrap();
    assert!(
        signature.contains(&format!("keyId=\"{expected_key_id}\"")),
        "signature must be made with the Profile key: {signature}"
    );

    // A Profile without a key cannot sign.
    let keyless = create_account(&env, None).await.unwrap();
    let err = env
        .module
        .sign_as_profile(
            &keyless.profile_id,
            HttpSigningRequest {
                method: "POST".to_string(),
                url: "https://remote.example/inbox".to_string(),
                headers: HashMap::new(),
                body: None,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(context(&err), &KernelError::NotFound);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn actor_document_and_webfinger_point_at_the_profile() {
    let env = env().await;
    let username = unique_username();
    let created = create_account(&env, Some(username.clone())).await.unwrap();
    let actor_dto = || GetProfileActorDto {
        profile_nanoid: created.profile_nanoid.as_ref().to_string(),
    };
    let webfinger_dto = |username: &str| GetProfileWebFingerDto {
        username: username.to_string(),
        domain: "Example.COM".to_string(),
    };

    // A Profile is not an actor before it has a key.
    let err = env.module.get_profile_actor(actor_dto()).await.unwrap_err();
    assert_eq!(context(&err), &KernelError::NotFound);
    let err = env
        .module
        .get_profile_webfinger(webfinger_dto(&username))
        .await
        .unwrap_err();
    assert_eq!(context(&err), &KernelError::NotFound);

    let key = create_profile_key(&env, &created).await.unwrap();
    let actor_id = format!(
        "https://example.com/ap/profiles/{}",
        created.profile_nanoid.as_ref()
    );

    let actor = env.module.get_profile_actor(actor_dto()).await.unwrap();
    assert_eq!(actor.id, actor_id);
    assert_eq!(actor.url, actor_id);
    assert_eq!(actor.preferred_username, username);
    assert_eq!(actor.inbox, format!("{actor_id}/inbox"));
    assert_eq!(actor.outbox, format!("{actor_id}/outbox"));
    assert_eq!(actor.followers, format!("{actor_id}/followers"));
    assert_eq!(actor.following, format!("{actor_id}/following"));
    assert_eq!(actor.public_key.id, format!("{actor_id}#main-key"));
    assert_eq!(actor.public_key.owner, actor_id);
    assert_eq!(actor.public_key.public_key_pem, key.public_key_pem);

    let webfinger = env
        .module
        .get_profile_webfinger(webfinger_dto(&username))
        .await
        .unwrap();
    assert_eq!(webfinger.subject, format!("acct:{username}@example.com"));
    let links = webfinger.links.unwrap();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].rel, "self");
    assert_eq!(links[0].href, actor_id);
    assert_eq!(webfinger.aliases.unwrap(), vec![actor_id.clone()]);

    let err = env
        .module
        .get_profile_webfinger(webfinger_dto("no-such-user-here"))
        .await
        .unwrap_err();
    assert_eq!(context(&err), &KernelError::NotFound);
    let err = env
        .module
        .get_profile_webfinger(webfinger_dto("Not Valid"))
        .await
        .unwrap_err();
    assert_eq!(context(&err), &KernelError::NotFound);

    // A Profile with a key but no username is still not an actor.
    let nameless = create_account(&env, None).await.unwrap();
    create_profile_key(&env, &nameless).await.unwrap();
    let err = env
        .module
        .get_profile_actor(GetProfileActorDto {
            profile_nanoid: nameless.profile_nanoid.as_ref().to_string(),
        })
        .await
        .unwrap_err();
    assert_eq!(context(&err), &KernelError::NotFound);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn profile_transfer_keeps_username_actor_uri_and_key_owner() {
    let env = env().await;
    let username = unique_username();
    let created = create_account(&env, Some(username.clone())).await.unwrap();
    let key = create_profile_key(&env, &created).await.unwrap();
    let actor_before = env
        .module
        .get_profile_actor(GetProfileActorDto {
            profile_nanoid: created.profile_nanoid.as_ref().to_string(),
        })
        .await
        .unwrap();

    // Organization account that receives the Profile (same aggregate commands as the
    // profile-transfer accept use case: AccountTransferred event + read-model update).
    let org_id = AccountId::default();
    let mut conn = env.module.database.connection().await.unwrap();
    let event = env
        .module
        .account_repository()
        .save(
            &mut conn,
            Account::create_organization(
                org_id.clone(),
                AccountName::new(format!("org-{}", org_id.as_ref())),
                AccountIsBot::new(false),
                Nanoid::default(),
                env.auth_account_id.clone(),
            ),
        )
        .await
        .unwrap();
    let mut org = None;
    kernel::interfaces::event::EventApplier::apply(&mut org, event).unwrap();
    env.module
        .account_read_model()
        .create(&mut conn, &org.unwrap())
        .await
        .unwrap();
    let from_account: i64 = sqlx::query_scalar("SELECT account_id FROM profiles WHERE id = $1")
        .bind(created.profile_id.as_ref())
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    env.module
        .profile_repository()
        .save(
            &mut conn,
            Profile::transfer_account(
                created.profile_id.clone(),
                AccountId::new(from_account),
                org_id.clone(),
            ),
        )
        .await
        .unwrap();
    let rehydrated = env
        .module
        .profile_repository()
        .load(&mut conn, &created.profile_id)
        .await
        .unwrap();
    env.module
        .profile_read_model()
        .update(&mut conn, rehydrated.aggregate())
        .await
        .unwrap();
    drop(conn);

    let owner_after: i64 = sqlx::query_scalar("SELECT account_id FROM profiles WHERE id = $1")
        .bind(created.profile_id.as_ref())
        .fetch_one(&mut *env.module.database.connection().await.unwrap())
        .await
        .unwrap();
    assert_eq!(owner_after, *org_id.as_ref(), "ownership moved");
    assert_eq!(
        stored_username(&env, &created.profile_id).await.as_deref(),
        Some(username.as_str()),
        "username survives the transfer"
    );
    assert_eq!(rehydrated.aggregate().nanoid(), &created.profile_nanoid);

    let mut conn = env.module.database.connection().await.unwrap();
    let keys = env
        .module
        .profile_signing_key_repository()
        .find_active_by_profile_id(&mut conn, &created.profile_id)
        .await
        .unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(
        keys[0].id(),
        key.id(),
        "the same key stays with the Profile"
    );
    drop(conn);

    let actor_after = env
        .module
        .get_profile_actor(GetProfileActorDto {
            profile_nanoid: created.profile_nanoid.as_ref().to_string(),
        })
        .await
        .unwrap();
    assert_eq!(actor_after.id, actor_before.id);
    assert_eq!(actor_after.preferred_username, username);
    assert_eq!(actor_after.public_key.public_key_pem, key.public_key_pem);
    assert_eq!(actor_after.public_key.id, actor_before.public_key.id);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn rehydrating_the_event_stream_restores_the_username() {
    let env = env().await;
    let username = unique_username();
    let created = create_account(&env, None).await.unwrap();
    env.module
        .set_profile_username(
            &env.auth_account_id,
            None,
            created.profile_nanoid.as_ref().to_string(),
            username.clone(),
        )
        .await
        .unwrap();

    let mut conn = env.module.database.connection().await.unwrap();
    let rehydrated = env
        .module
        .profile_repository()
        .load(&mut conn, &created.profile_id)
        .await
        .unwrap();
    assert_eq!(
        rehydrated
            .aggregate()
            .username()
            .as_ref()
            .map(|u| u.as_ref().as_str()),
        Some(username.as_str())
    );
}

async fn create_organization(env: &Env) -> AccountId {
    let org_id = AccountId::default();
    let mut conn = env.module.database.connection().await.unwrap();
    let event = env
        .module
        .account_repository()
        .save(
            &mut conn,
            Account::create_organization(
                org_id.clone(),
                AccountName::new(format!("org-{}", org_id.as_ref())),
                AccountIsBot::new(false),
                Nanoid::default(),
                env.auth_account_id.clone(),
            ),
        )
        .await
        .unwrap();
    let mut org = None;
    kernel::interfaces::event::EventApplier::apply(&mut org, event).unwrap();
    env.module
        .account_read_model()
        .create(&mut conn, &org.unwrap())
        .await
        .unwrap();
    org_id
}

/// Waits until some backend is blocked by the backend `blocker_pid`.
async fn wait_until_blocked_by(env: &Env, blocker_pid: i32) {
    for _ in 0..200 {
        let blocked: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid))",
        )
        .bind(blocker_pid)
        .fetch_one(&mut *env.module.database.connection().await.unwrap())
        .await
        .unwrap();
        if blocked > 0 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("the username request never blocked on the Profile row lock");
}

/// R1: a username request that was authorized for the previous owner must not set the immutable
/// username on a Profile that moved to somebody else in the meantime. The transfer's row lock is
/// held (uncommitted) while the request is started, so the request has passed its pre-check
/// against the old owner and then waits for the lock; it resumes only after the transfer committed.
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn username_request_authorized_for_the_old_owner_is_refused_after_a_concurrent_transfer() {
    let env = env().await;
    let created = create_account(&env, None).await.unwrap();
    let org_id = create_organization(&env).await;
    let from_account: i64 = sqlx::query_scalar("SELECT account_id FROM profiles WHERE id = $1")
        .bind(created.profile_id.as_ref())
        .fetch_one(&mut *env.module.database.connection().await.unwrap())
        .await
        .unwrap();

    // The transfer: same aggregate commands as the accept use case, left uncommitted.
    let mut transfer = env.module.database.get_transaction().await.unwrap();
    let transfer_pid: i32 = {
        let con: &mut sqlx::PgConnection = transfer.connection();
        sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(con)
            .await
            .unwrap()
    };
    {
        let executor = transfer.connection();
        env.module
            .profile_repository()
            .save(
                executor,
                Profile::transfer_account(
                    created.profile_id.clone(),
                    AccountId::new(from_account),
                    org_id.clone(),
                ),
            )
            .await
            .unwrap();
        let rehydrated = env
            .module
            .profile_repository()
            .load(executor, &created.profile_id)
            .await
            .unwrap();
        env.module
            .profile_read_model()
            .update(executor, rehydrated.aggregate())
            .await
            .unwrap();
    }

    let module = env.module.clone();
    let auth = env.auth_account_id.clone();
    let nanoid = created.profile_nanoid.as_ref().to_string();
    let username = unique_username();
    let request = {
        let username = username.clone();
        tokio::spawn(async move {
            module
                .set_profile_username(&auth, None, nanoid, username)
                .await
                .map(|_| ())
        })
    };
    wait_until_blocked_by(&env, transfer_pid).await;
    assert!(!request.is_finished(), "the request waits for the transfer");
    transfer.commit().await.unwrap();

    let error = request.await.unwrap().unwrap_err();
    assert_eq!(context(&error), &KernelError::Concurrency);
    assert_eq!(stored_username(&env, &created.profile_id).await, None);
    assert_eq!(
        count(
            &env,
            "SELECT count(*) FROM profile_events WHERE id = $1 AND data ->> 'username' IS NOT NULL",
            *created.profile_id.as_ref(),
        )
        .await,
        0,
        "no username event may exist for the transferred Profile"
    );
    let owner: i64 = sqlx::query_scalar("SELECT account_id FROM profiles WHERE id = $1")
        .bind(created.profile_id.as_ref())
        .fetch_one(&mut *env.module.database.connection().await.unwrap())
        .await
        .unwrap();
    assert_eq!(owner, *org_id.as_ref());

    // The new owner (the organization context) can set it.
    let context_of_new_owner = OrganizationContext {
        org_account_id: org_id,
        org_account_nanoid: "new-owner".to_string(),
        role: OrgRole::Admin,
    };
    let dto = env
        .module
        .set_profile_username(
            &env.auth_account_id,
            Some(&context_of_new_owner),
            created.profile_nanoid.as_ref().to_string(),
            username.clone(),
        )
        .await
        .unwrap();
    assert_eq!(dto.username.as_deref(), Some(username.as_str()));
}

/// The other order: the username request holds the Profile lock, the transfer waits and then
/// moves the Profile with its freshly set username intact.
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn transfer_waiting_behind_a_username_write_keeps_the_username() {
    let env = env().await;
    let created = create_account(&env, None).await.unwrap();
    let org_id = create_organization(&env).await;
    let username = unique_username();
    let from_account: i64 = sqlx::query_scalar("SELECT account_id FROM profiles WHERE id = $1")
        .bind(created.profile_id.as_ref())
        .fetch_one(&mut *env.module.database.connection().await.unwrap())
        .await
        .unwrap();

    // The setter's write transaction, left open after the locked re-read + event + projection.
    let mut setter = env.module.database.get_transaction().await.unwrap();
    let setter_pid: i32 = {
        let con: &mut sqlx::PgConnection = setter.connection();
        sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(con)
            .await
            .unwrap()
    };
    let username_value = kernel::prelude::entity::ProfileUsername::new(username.clone());
    {
        let executor = setter.connection();
        env.module
            .profile_read_model()
            .lock_by_id_for_update(executor, &created.profile_id)
            .await
            .unwrap()
            .unwrap();
        let event = env
            .module
            .profile_repository()
            .save(
                executor,
                Profile::set_username(created.profile_id.clone(), username_value.clone()),
            )
            .await
            .unwrap();
        env.module
            .profile_read_model()
            .set_username(
                executor,
                &created.profile_id,
                &username_value,
                &event.version,
            )
            .await
            .unwrap();
    }

    let module = env.module.clone();
    let profile_id = created.profile_id.clone();
    let transfer = tokio::spawn(async move {
        let mut conn = module.database.connection().await.unwrap();
        module
            .profile_repository()
            .save(
                &mut conn,
                Profile::transfer_account(profile_id.clone(), AccountId::new(from_account), org_id),
            )
            .await
            .unwrap();
        let rehydrated = module
            .profile_repository()
            .load(&mut conn, &profile_id)
            .await
            .unwrap();
        module
            .profile_read_model()
            .update(&mut conn, rehydrated.aggregate())
            .await
            .unwrap();
    });
    wait_until_blocked_by(&env, setter_pid).await;
    setter.commit().await.unwrap();
    transfer.await.unwrap();

    assert_eq!(
        stored_username(&env, &created.profile_id).await.as_deref(),
        Some(username.as_str())
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn suspended_owner_gets_a_state_error_not_a_missing_account() {
    let env = env().await;
    let created = create_account(&env, None).await.unwrap();
    // What the Account projector writes for an `AccountEvent::Suspended`.
    sqlx::query(
        "UPDATE accounts SET suspended_at = NOW(), suspend_reason = 'test suspension' WHERE nanoid = $1",
    )
    .bind(&created.account_nanoid)
    .execute(&mut *env.module.database.connection().await.unwrap())
    .await
    .unwrap();
    let error = env
        .module
        .set_profile_username(
            &env.auth_account_id,
            None,
            created.profile_nanoid.as_ref().to_string(),
            unique_username(),
        )
        .await
        .unwrap_err();
    assert_eq!(context(&error), &KernelError::Rejected);
    assert_eq!(stored_username(&env, &created.profile_id).await, None);
}
