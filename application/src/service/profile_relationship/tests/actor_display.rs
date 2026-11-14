//! 個別 block・mute 一覧の対象 actor の表示情報 (D08 ui-spec 1-6) の受け入れテスト。実 DB と fake peer。
//!
//! 一覧・作成の処理中はリモートに接続せず、取得は `refresh_remote_actor_profiles` (応答本体を取り出し終えた後に呼ぶ) だけが行う。

use super::support::{Env, User, LOCAL_BASE};
use crate::dto::profile_relationship::{
    ProfileRelationItemDto, RelationActorKind, RelationActorStatus,
};
use crate::service::profile_relation_actor::{
    ProfileRelationActorUseCase, REMOTE_ACTOR_REFRESH_BATCH, REMOTE_ACTOR_REFRESH_TTL,
    REMOTE_ACTOR_RETRY_AFTER,
};
use crate::service::profile_relation_list::{
    ProfileRelationListKind, ProfileRelationListUseCase, ProfileRelationPageDto,
};
use crate::service::profile_relationship::ProfileRelationshipUseCase;
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::read_model::{DependOnProfileReadModel, ProfileReadModel};
use kernel::interfaces::repository::{DependOnImageRepository, ImageRepository};
use kernel::prelude::entity::{ImageId, Nanoid, ProfileId};
use kernel::test_utils::{ImageBuilder, ProfileBuilder};
use kernel::KernelError;
use time::{Duration, OffsetDateTime};
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

async fn mute(env: &Env, user: &User, target: &str) -> ProfileRelationItemDto {
    let relation = env
        .module
        .mute_actor(&user.auth, None, &user.nanoid, target)
        .await
        .unwrap();
    ProfileRelationItemDto {
        target_actor: env
            .module
            .describe_relation_targets(std::slice::from_ref(&relation.target))
            .await
            .0
            .remove(0),
        relation,
    }
}

async fn list(
    env: &Env,
    user: &User,
    kind: ProfileRelationListKind,
    limit: Option<u32>,
) -> ProfileRelationPageDto {
    env.module
        .list_profile_relations(&user.auth, None, &user.nanoid, kind, None, limit)
        .await
        .unwrap()
}

async fn list_mutes(env: &Env, user: &User) -> ProfileRelationPageDto {
    list(env, user, ProfileRelationListKind::Mute, None).await
}

/// peer への `GET {path}` の回数。
async fn gets(env: &Env, actor_path: &str) -> usize {
    env.peer
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|request| request.method.as_str() == "GET" && request.url.path() == actor_path)
        .count()
}

async fn gets_total(env: &Env) -> usize {
    env.peer
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|request| request.method.as_str() == "GET")
        .count()
}

/// 表示情報を持つ actor 文書を `GET {actor_path}` で公開する。
async fn mount_person(env: &Env, actor_path: &str, username: &str, name: &str) -> String {
    let id = env.peer_url(actor_path);
    Mock::given(method("GET"))
        .and(path(actor_path))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": id,
            "type": "Person",
            "inbox": env.peer_url("/inbox"),
            "preferredUsername": username,
            "name": name,
            "icon": {"type": "Image", "url": format!("https://cdn.example/{username}.png")},
        })))
        .mount(&env.peer)
        .await;
    id
}

async fn mount_status(env: &Env, actor_path: &str, status: u16) -> String {
    Mock::given(method("GET"))
        .and(path(actor_path))
        .respond_with(ResponseTemplate::new(status))
        .mount(&env.peer)
        .await;
    env.peer_url(actor_path)
}

fn peer_host(env: &Env) -> String {
    env.peer.address().to_string()
}

async fn seed_described_local_profile(
    env: &Env,
    username: Option<&str>,
    display_name: Option<&str>,
    icon_url: Option<&str>,
) -> (ProfileId, String) {
    let account = env.seed_organization().await;
    let mut executor = env.iso.database.connection().await.unwrap();
    let icon = match icon_url {
        Some(url) => {
            let image = ImageBuilder::new()
                .id(ImageId::new(kernel::generate_id()))
                .url(url)
                .build();
            env.module
                .image_repository()
                .create(&mut executor, &image)
                .await
                .unwrap();
            Some(image.id().clone())
        }
        None => None,
    };
    let mut builder = ProfileBuilder::new()
        .id(ProfileId::new(kernel::generate_id()))
        .account_id(account.id().clone())
        .nanoid(Nanoid::default())
        .display_name(display_name)
        .icon(icon);
    if let Some(username) = username {
        builder = builder.username(username);
    }
    let profile = builder.build();
    env.module
        .profile_read_model()
        .create(&mut executor, &profile)
        .await
        .unwrap();
    (profile.id().clone(), profile.nanoid().as_ref().to_string())
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_local_target_is_described_from_its_profile_in_the_same_shape() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    let (_, carol) = seed_described_local_profile(
        &env,
        Some("carol"),
        Some("Carol"),
        Some("https://media.local.example/carol.png"),
    )
    .await;
    let (_, dave) = seed_described_local_profile(&env, None, None, None).await;
    // A local target is accepted as a bare Profile nanoid or as its actor URL.
    mute(&env, &alice, &carol).await;
    let created = mute(&env, &alice, &format!("{LOCAL_BASE}/ap/profiles/{dave}")).await;
    // The response of the create path describes the target the same way.
    assert_eq!(created.target_actor.kind, RelationActorKind::Local);
    assert_eq!(created.target_actor.status, RelationActorStatus::Resolved);

    let before = gets_total(&env).await;
    let page = list_mutes(&env, &alice).await;
    assert_eq!(gets_total(&env).await, before, "no remote access");
    assert!(page.refresh.is_empty());
    assert_eq!(page.items.len(), 2);

    let carol_item = &page.items[0];
    assert_eq!(
        carol_item.relation.target,
        format!("{LOCAL_BASE}/ap/profiles/{carol}")
    );
    let actor = &carol_item.target_actor;
    assert_eq!(actor.kind, RelationActorKind::Local);
    assert_eq!(actor.actor_url, carol_item.relation.target);
    assert_eq!(actor.profile_nanoid.as_deref(), Some(carol.as_str()));
    assert_eq!(actor.status, RelationActorStatus::Resolved);
    assert_eq!(actor.acct.as_deref(), Some("carol@local.example"));
    assert_eq!(actor.preferred_username.as_deref(), Some("carol"));
    assert_eq!(actor.display_name.as_deref(), Some("Carol"));
    assert_eq!(
        actor.icon_url.as_deref(),
        Some("https://media.local.example/carol.png")
    );

    // A Profile without username / name / icon is still resolved; the missing parts are null.
    let dave_actor = &page.items[1].target_actor;
    assert_eq!(dave_actor.profile_nanoid.as_deref(), Some(dave.as_str()));
    assert_eq!(dave_actor.status, RelationActorStatus::Resolved);
    assert_eq!(dave_actor.acct, None);
    assert_eq!(dave_actor.display_name, None);
    assert_eq!(dave_actor.icon_url, None);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_remote_target_is_pending_until_fetched_and_the_list_never_contacts_the_peer() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    let bob_url = mount_person(&env, "/users/bob", "bob", "Bob Builder").await;
    let created = mute(&env, &alice, &bob_url).await;
    assert_eq!(created.target_actor.kind, RelationActorKind::Remote);
    assert_eq!(created.target_actor.status, RelationActorStatus::Pending);
    assert_eq!(created.target_actor.actor_url, bob_url);
    assert_eq!(created.target_actor.profile_nanoid, None);

    for _ in 0..3 {
        let page = list_mutes(&env, &alice).await;
        assert_eq!(page.items.len(), 1);
        let actor = &page.items[0].target_actor;
        assert_eq!(actor.status, RelationActorStatus::Pending);
        assert_eq!(page.items[0].relation.target, bob_url, "fallback URL stays");
        assert_eq!(
            (
                &actor.acct,
                &actor.preferred_username,
                &actor.display_name,
                &actor.icon_url
            ),
            (&None, &None, &None, &None)
        );
    }
    assert_eq!(gets(&env, "/users/bob").await, 0, "no synchronous fetch");

    // The background refresh fills the cache; the next read has the data.
    let page = list_mutes(&env, &alice).await;
    assert_eq!(page.refresh, vec![bob_url.clone()]);
    env.module
        .refresh_remote_actor_profiles(&page.refresh)
        .await;
    assert_eq!(gets(&env, "/users/bob").await, 1);

    let page = list_mutes(&env, &alice).await;
    assert!(page.refresh.is_empty(), "fresh data is not refreshed");
    let actor = &page.items[0].target_actor;
    assert_eq!(actor.status, RelationActorStatus::Resolved);
    assert_eq!(
        actor.acct.as_deref(),
        Some(format!("bob@{}", peer_host(&env)).as_str())
    );
    assert_eq!(actor.preferred_username.as_deref(), Some("bob"));
    assert_eq!(actor.display_name.as_deref(), Some("Bob Builder"));
    assert_eq!(
        actor.icon_url.as_deref(),
        Some("https://cdn.example/bob.png")
    );
    assert_eq!(gets(&env, "/users/bob").await, 1, "the read did not fetch");
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn failed_fetches_are_unavailable_and_retried_only_after_the_backoff() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    let gone = mount_status(&env, "/users/gone", 404).await;
    let broken = mount_status(&env, "/users/broken", 500).await;
    // An actor document that names another actor, and one without a usable preferredUsername.
    let spoof_path = "/users/spoof";
    Mock::given(method("GET"))
        .and(path(spoof_path))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "https://elsewhere.example/users/real", "preferredUsername": "real",
        })))
        .mount(&env.peer)
        .await;
    let spoof = env.peer_url(spoof_path);
    let nameless = env.peer_url("/users/nameless");
    Mock::given(method("GET"))
        .and(path("/users/nameless"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": nameless, "preferredUsername": "has space@and/slash",
        })))
        .mount(&env.peer)
        .await;
    let targets = [&gone, &broken, &spoof, &nameless];
    for target in targets {
        mute(&env, &alice, target).await;
    }

    let page = list_mutes(&env, &alice).await;
    assert_eq!(page.refresh.len(), 4);
    env.module
        .refresh_remote_actor_profiles(&page.refresh)
        .await;

    let page = list_mutes(&env, &alice).await;
    assert_eq!(page.items.len(), 4);
    for item in &page.items {
        let actor = &item.target_actor;
        assert_eq!(
            actor.status,
            RelationActorStatus::Unavailable,
            "{}",
            item.relation.target
        );
        assert_eq!(
            (
                &actor.acct,
                &actor.preferred_username,
                &actor.display_name,
                &actor.icon_url
            ),
            (&None, &None, &None, &None)
        );
        assert_eq!(actor.actor_url, item.relation.target);
    }
    assert!(
        page.refresh.is_empty(),
        "inside the backoff nothing is refreshed"
    );
    let fetches = gets_total(&env).await;

    // Still inside the backoff window: a refresh request does not fetch again.
    env.module
        .refresh_remote_actor_profiles_at(
            &targets.map(|target| target.to_string()),
            OffsetDateTime::now_utc() + REMOTE_ACTOR_RETRY_AFTER - Duration::minutes(1),
        )
        .await;
    assert_eq!(gets_total(&env).await, fetches);

    // After the backoff the peer has recovered: the retry succeeds.
    env.peer.reset().await;
    mount_person(&env, "/users/gone", "gone", "Back Again").await;
    env.module
        .refresh_remote_actor_profiles_at(
            std::slice::from_ref(&gone),
            OffsetDateTime::now_utc() + REMOTE_ACTOR_RETRY_AFTER + Duration::minutes(1),
        )
        .await;
    let page = list_mutes(&env, &alice).await;
    let recovered = page
        .items
        .iter()
        .find(|item| item.relation.target == gone)
        .unwrap();
    assert_eq!(recovered.target_actor.status, RelationActorStatus::Resolved);
    assert_eq!(
        recovered.target_actor.display_name.as_deref(),
        Some("Back Again")
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_failed_refresh_keeps_serving_the_old_data_until_a_refresh_succeeds() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    let bob_url = mount_person(&env, "/users/bob", "bob", "Old Name").await;
    mute(&env, &alice, &bob_url).await;
    env.module
        .refresh_remote_actor_profiles(std::slice::from_ref(&bob_url))
        .await;
    assert_eq!(
        list_mutes(&env, &alice).await.items[0]
            .target_actor
            .display_name
            .as_deref(),
        Some("Old Name")
    );

    // Not due yet: nothing is fetched.
    let before = gets(&env, "/users/bob").await;
    env.module
        .refresh_remote_actor_profiles_at(
            std::slice::from_ref(&bob_url),
            OffsetDateTime::now_utc() + REMOTE_ACTOR_REFRESH_TTL - Duration::minutes(1),
        )
        .await;
    assert_eq!(gets(&env, "/users/bob").await, before);

    // Due, but the peer is down: the old data stays resolved.
    env.peer.reset().await;
    mount_status(&env, "/users/bob", 503).await;
    let due = OffsetDateTime::now_utc() + REMOTE_ACTOR_REFRESH_TTL + Duration::minutes(1);
    env.module
        .refresh_remote_actor_profiles_at(std::slice::from_ref(&bob_url), due)
        .await;
    assert_eq!(gets(&env, "/users/bob").await, 1);
    let actor = list_mutes(&env, &alice).await.items.remove(0).target_actor;
    assert_eq!(actor.status, RelationActorStatus::Resolved);
    assert_eq!(actor.display_name.as_deref(), Some("Old Name"));

    // After the retry delay the peer is back with a new name: the data is replaced.
    env.peer.reset().await;
    mount_person(&env, "/users/bob", "bob", "New Name").await;
    env.module
        .refresh_remote_actor_profiles_at(
            std::slice::from_ref(&bob_url),
            due + REMOTE_ACTOR_RETRY_AFTER + Duration::minutes(1),
        )
        .await;
    let actor = list_mutes(&env, &alice).await.items.remove(0).target_actor;
    assert_eq!(actor.display_name.as_deref(), Some("New Name"));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn simultaneous_refresh_requests_fetch_the_actor_once() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    let bob_url = mount_person(&env, "/users/bob", "bob", "Bob").await;
    mute(&env, &alice, &bob_url).await;
    let urls = vec![bob_url.clone()];
    let mut handles = Vec::new();
    for _ in 0..8 {
        let module = env.module.clone();
        let urls = urls.clone();
        handles.push(tokio::spawn(async move {
            module.refresh_remote_actor_profiles(&urls).await;
        }));
    }
    for handle in handles {
        handle.await.unwrap();
    }
    assert_eq!(gets(&env, "/users/bob").await, 1);
    assert_eq!(
        list_mutes(&env, &alice).await.items[0].target_actor.status,
        RelationActorStatus::Resolved
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn one_list_schedules_at_most_ten_actors_and_the_rest_follow() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    let total = REMOTE_ACTOR_REFRESH_BATCH + 3;
    for index in 0..total {
        let url = mount_person(&env, &format!("/users/u{index}"), &format!("u{index}"), "U").await;
        mute(&env, &alice, &url).await;
    }
    let page = list(&env, &alice, ProfileRelationListKind::Mute, Some(100)).await;
    assert_eq!(page.items.len(), total);
    assert_eq!(page.refresh.len(), REMOTE_ACTOR_REFRESH_BATCH);
    // Oldest first, like the list.
    assert_eq!(
        page.refresh,
        page.items
            .iter()
            .take(REMOTE_ACTOR_REFRESH_BATCH)
            .map(|item| item.relation.target.clone())
            .collect::<Vec<_>>()
    );
    env.module
        .refresh_remote_actor_profiles(&page.refresh)
        .await;

    let page = list(&env, &alice, ProfileRelationListKind::Mute, Some(100)).await;
    assert_eq!(page.refresh.len(), 3);
    env.module
        .refresh_remote_actor_profiles(&page.refresh)
        .await;
    let page = list(&env, &alice, ProfileRelationListKind::Mute, Some(100)).await;
    assert!(page.refresh.is_empty());
    assert!(page
        .items
        .iter()
        .all(|item| item.target_actor.status == RelationActorStatus::Resolved));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn the_cache_is_shared_but_each_owner_only_sees_their_own_settings() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    let bob = env.seed_user().await;
    let carol = env.seed_user().await;
    let eve_url = mount_person(&env, "/users/eve", "eve", "Eve").await;
    mute(&env, &alice, &eve_url).await;
    mute(&env, &bob, &eve_url).await;
    env.module
        .refresh_remote_actor_profiles(std::slice::from_ref(&eve_url))
        .await;

    // Bob reads the data fetched for Alice's request: no second fetch.
    let bobs = list_mutes(&env, &bob).await;
    assert_eq!(
        bobs.items[0].target_actor.status,
        RelationActorStatus::Resolved
    );
    assert!(bobs.refresh.is_empty());
    assert_eq!(gets(&env, "/users/eve").await, 1);
    // Carol muted nothing; Alice's setting does not leak into her list.
    assert!(list_mutes(&env, &carol).await.items.is_empty());
    // A block list only holds blocks.
    assert!(list(&env, &alice, ProfileRelationListKind::Block, None)
        .await
        .items
        .is_empty());
    // Only the owner can read a Profile's list.
    let denied = env
        .module
        .list_profile_relations(
            &bob.auth,
            None,
            &alice.nanoid,
            ProfileRelationListKind::Mute,
            None,
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(
        denied.current_context(),
        KernelError::PermissionDenied | KernelError::NotFound
    ));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_block_is_described_like_a_mute_and_keeps_its_fields() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    let bob_url = mount_person(&env, "/users/bob", "bob", "Bob").await;
    let block = env
        .module
        .block_actor(&alice.auth, None, &alice.nanoid, &bob_url)
        .await
        .unwrap();
    env.module
        .refresh_remote_actor_profiles(std::slice::from_ref(&block.target))
        .await;
    let page = list(&env, &alice, ProfileRelationListKind::Block, None).await;
    assert_eq!(page.items.len(), 1);
    let item = &page.items[0];
    // The existing fields are unchanged.
    assert_eq!(item.relation.nanoid, block.nanoid);
    assert_eq!(item.relation.profile_nanoid, alice.nanoid);
    assert_eq!(item.relation.target, bob_url);
    assert_eq!(item.relation.created_at, block.created_at);
    assert_eq!(item.target_actor.status, RelationActorStatus::Resolved);
    assert_eq!(item.target_actor.display_name.as_deref(), Some("Bob"));
}

/// 応答のたびに仮想時計を進める peer の応答 (遅い取得の再現。実時間は待たない)。
struct AdvanceClock {
    offset_seconds: std::sync::Arc<std::sync::atomic::AtomicI64>,
    advance_to: i64,
    body: serde_json::Value,
    delay: std::time::Duration,
}

impl wiremock::Respond for AdvanceClock {
    fn respond(&self, _: &wiremock::Request) -> ResponseTemplate {
        self.offset_seconds
            .store(self.advance_to, std::sync::atomic::Ordering::SeqCst);
        ResponseTemplate::new(200)
            .set_body_json(&self.body)
            .set_delay(self.delay)
    }
}

async fn next_refresh_at(env: &Env, url: &str) -> OffsetDateTime {
    sqlx::query_scalar(
        "SELECT next_refresh_at FROM profile_remote_actor_cache WHERE actor_url = $1",
    )
    .bind(url)
    .fetch_one(&env.iso.pool)
    .await
    .unwrap()
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn each_actor_uses_the_time_of_its_own_claim_and_record_not_the_start_of_the_batch() {
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::Arc;

    let env = Env::new().await;
    let alice = env.seed_user().await;
    let first = env.peer_url("/users/first");
    let second = env.peer_url("/users/second");
    mute(&env, &alice, &first).await;
    mute(&env, &alice, &second).await;

    // 仮想時計: 基準時刻 + offset。first の取得で 50 秒、second の取得中に 100 秒まで進む。
    let base = OffsetDateTime::now_utc();
    let offset = Arc::new(AtomicI64::new(0));
    let doc = |url: &str, name: &str| serde_json::json!({"id": url, "type": "Person", "preferredUsername": name, "name": name});
    Mock::given(method("GET"))
        .and(path("/users/first"))
        .respond_with(AdvanceClock {
            offset_seconds: offset.clone(),
            advance_to: 50,
            body: doc(&first, "first"),
            delay: std::time::Duration::ZERO,
        })
        .mount(&env.peer)
        .await;
    Mock::given(method("GET"))
        .and(path("/users/second"))
        .respond_with(AdvanceClock {
            offset_seconds: offset.clone(),
            advance_to: 100,
            body: doc(&second, "second"),
            delay: std::time::Duration::from_millis(1500),
        })
        .mount(&env.peer)
        .await;

    let urls = vec![first.clone(), second.clone()];
    let batch = {
        let module = env.module.clone();
        let offset = offset.clone();
        let urls = urls.clone();
        tokio::spawn(async move {
            let clock = move || base + Duration::seconds(offset.load(Ordering::SeqCst));
            module
                .refresh_remote_actor_profiles_with_clock(&urls, &clock)
                .await;
        })
    };
    // second の取得が始まる (= first の記録が済み、second のリースが確保された) まで待つ。
    for _ in 0..200 {
        if gets(&env, "/users/second").await > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(gets(&env, "/users/second").await, 1);

    // 取得の途中 (仮想時刻 100 秒) で一覧が再読込され、同じ actor が要求される。second のリースは
    // 確保した時刻 (50 秒) から 60 秒間 = 110 秒まで有効なので、2 回目の取得は始まらない。
    offset.store(100, Ordering::SeqCst);
    let mid_batch_clock = || base + Duration::seconds(100);
    env.module
        .refresh_remote_actor_profiles_with_clock(std::slice::from_ref(&second), &mid_batch_clock)
        .await;
    batch.await.unwrap();
    assert_eq!(gets(&env, "/users/second").await, 1, "the lease still held");
    assert_eq!(gets(&env, "/users/first").await, 1);

    // 期限は actor ごとの記録時刻から数える (バッチ開始時刻からではない)。
    let close = |actual: OffsetDateTime, expected: OffsetDateTime| {
        (actual - expected).abs() < Duration::seconds(2)
    };
    let first_due = next_refresh_at(&env, &first).await;
    assert!(
        close(
            first_due,
            base + Duration::seconds(50) + REMOTE_ACTOR_REFRESH_TTL
        ),
        "{first_due}"
    );
    let second_due = next_refresh_at(&env, &second).await;
    assert!(
        close(
            second_due,
            base + Duration::seconds(100) + REMOTE_ACTOR_REFRESH_TTL
        ),
        "{second_due}"
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_failed_fetch_is_backed_off_from_the_time_it_finished() {
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::Arc;

    let env = Env::new().await;
    let alice = env.seed_user().await;
    let down = env.peer_url("/users/down");
    mute(&env, &alice, &down).await;
    let base = OffsetDateTime::now_utc();
    let offset = Arc::new(AtomicI64::new(0));
    // 取得が 40 秒かかって失敗する。
    Mock::given(method("GET"))
        .and(path("/users/down"))
        .respond_with({
            struct Fail(Arc<AtomicI64>);
            impl wiremock::Respond for Fail {
                fn respond(&self, _: &wiremock::Request) -> ResponseTemplate {
                    self.0.store(40, Ordering::SeqCst);
                    ResponseTemplate::new(503)
                }
            }
            Fail(offset.clone())
        })
        .mount(&env.peer)
        .await;
    let clock = {
        let offset = offset.clone();
        move || base + Duration::seconds(offset.load(Ordering::SeqCst))
    };
    env.module
        .refresh_remote_actor_profiles_with_clock(std::slice::from_ref(&down), &clock)
        .await;
    let due = next_refresh_at(&env, &down).await;
    let expected = base + Duration::seconds(40) + REMOTE_ACTOR_RETRY_AFTER;
    assert!((due - expected).abs() < Duration::seconds(2), "{due}");
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_preferred_username_with_bidi_or_invisible_characters_is_a_failed_fetch() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    let mut targets = Vec::new();
    for (index, username) in [
        "evil\u{202E}gpj",
        "a\u{2066}b",
        "a\u{200F}",
        "a\u{200B}b",
        "a\u{FEFF}",
    ]
    .into_iter()
    .enumerate()
    {
        let actor_path = format!("/users/bidi{index}");
        let url = env.peer_url(&actor_path);
        Mock::given(method("GET"))
            .and(path(actor_path.as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": url, "type": "Person", "preferredUsername": username, "name": "Mallory",
            })))
            .mount(&env.peer)
            .await;
        mute(&env, &alice, &url).await;
        targets.push(url);
    }
    env.module.refresh_remote_actor_profiles(&targets).await;
    let page = list_mutes(&env, &alice).await;
    assert_eq!(page.items.len(), targets.len());
    for item in &page.items {
        let actor = &item.target_actor;
        assert_eq!(actor.status, RelationActorStatus::Unavailable);
        assert_eq!(
            (&actor.acct, &actor.preferred_username, &actor.display_name),
            (&None, &None, &None),
            "{}",
            item.relation.target
        );
    }
}
