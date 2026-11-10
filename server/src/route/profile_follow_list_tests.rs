//! followers / following reads over HTTP with a real database (P3i). Local and remote fixtures,
//! E8 response shapes, pagination, owner-only authorization and Profile / Account isolation.

use crate::route::profile_graph_test_support::{GraphWorld, LocalProfile, RemotePeer, User};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use kernel::prelude::entity::{GraphActorUrl, ProfileGraphActor};
use serde_json::Value;
use tower::ServiceExt;

async fn get(world: &GraphWorld, uri: &str, token: Option<&str>) -> (StatusCode, Value) {
    let mut builder = Request::builder().method("GET").uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let response = world
        .follow_list_router()
        .oneshot(builder.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn remote(peer: &RemotePeer) -> ProfileGraphActor {
    ProfileGraphActor::Remote(GraphActorUrl::new(peer.actor_url.clone()).unwrap())
}

fn local(profile: &LocalProfile) -> ProfileGraphActor {
    ProfileGraphActor::Local(profile.id.clone())
}

struct Fixture {
    world: GraphWorld,
    owner: User,
    profile: LocalProfile,
}

async fn fixture() -> Fixture {
    let world = GraphWorld::new().await;
    let owner = world.new_user().await;
    let profile = world.create_profile(&owner, true).await;
    Fixture {
        world,
        owner,
        profile,
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn followers_mix_remote_and_local_followers_in_the_e8_shape() {
    let Fixture {
        world,
        owner,
        profile,
    } = fixture().await;
    let accepted_peer = RemotePeer::new();
    let pending_peer = RemotePeer::new();
    let local_follower = world.actor_profile().await;
    world
        .seed_follow(
            remote(&accepted_peer),
            local(&profile),
            &accepted_peer.activity_id("f1"),
            true,
        )
        .await;
    world
        .seed_follow(
            remote(&pending_peer),
            local(&profile),
            &pending_peer.activity_id("f1"),
            false,
        )
        .await;
    world
        .seed_follow(
            local(&local_follower),
            local(&profile),
            &format!("{}/follows/1", local_follower.actor_url),
            true,
        )
        .await;

    let (status, body) = get(
        &world,
        &format!("/api/v1/profiles/{}/followers", profile.nanoid),
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["next_cursor"], Value::Null);
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    for item in items {
        assert_eq!(item["profile_id"], profile.nanoid.as_str());
        assert!(item["id"].as_str().is_some_and(|id| !id.is_empty()));
        assert!(item["created_at"].as_str().is_some());
    }
    assert_eq!(items[0]["follower"]["kind"], "remote");
    assert_eq!(
        items[0]["follower"]["actor_url"],
        accepted_peer.actor_url.as_str()
    );
    assert_eq!(items[0]["follower"]["profile_id"], Value::Null);
    assert_eq!(items[0]["state"], "accepted");
    assert_eq!(items[1]["follower"]["kind"], "remote");
    assert_eq!(items[1]["state"], "pending");
    assert_eq!(items[2]["follower"]["kind"], "local");
    assert_eq!(
        items[2]["follower"]["actor_url"],
        local_follower.actor_url.as_str()
    );
    assert_eq!(
        items[2]["follower"]["profile_id"],
        local_follower.nanoid.as_str()
    );
    assert_eq!(items[2]["state"], "accepted");
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn following_lists_targets_by_canonical_actor_url() {
    let Fixture {
        world,
        owner,
        profile,
    } = fixture().await;
    let peer = RemotePeer::new();
    let local_target = world.actor_profile().await;
    world
        .seed_follow(
            local(&profile),
            remote(&peer),
            &format!("{}/follows/1", profile.actor_url),
            false,
        )
        .await;
    world
        .seed_follow(
            local(&profile),
            local(&local_target),
            &format!("{}/follows/2", profile.actor_url),
            true,
        )
        .await;
    // Followers of the Profile never leak into its following list.
    world
        .seed_follow(
            remote(&peer),
            local(&profile),
            &peer.activity_id("f9"),
            true,
        )
        .await;

    let (status, body) = get(
        &world,
        &format!("/api/v1/profiles/{}/following", profile.nanoid),
        Some(&owner.token),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["profile_id"], profile.nanoid.as_str());
    assert_eq!(items[0]["target"], peer.actor_url.as_str());
    assert_eq!(items[0]["state"], "pending");
    assert_eq!(items[1]["target"], local_target.actor_url.as_str());
    assert_eq!(items[1]["state"], "accepted");
    assert!(items[0].get("follower").is_none());
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn pagination_walks_every_follower_exactly_once() {
    let Fixture {
        world,
        owner,
        profile,
    } = fixture().await;
    let mut expected = Vec::new();
    for _ in 0..5 {
        let peer = RemotePeer::new();
        world
            .seed_follow(
                remote(&peer),
                local(&profile),
                &peer.activity_id("f1"),
                true,
            )
            .await;
        expected.push(peer.actor_url.clone());
    }
    let base = format!("/api/v1/profiles/{}/followers", profile.nanoid);

    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let uri = match &cursor {
            Some(cursor) => format!("{base}?limit=2&cursor={cursor}"),
            None => format!("{base}?limit=2"),
        };
        let (status, body) = get(&world, &uri, Some(&owner.token)).await;
        assert_eq!(status, StatusCode::OK);
        pages += 1;
        for item in body["items"].as_array().unwrap() {
            seen.push(item["follower"]["actor_url"].as_str().unwrap().to_string());
        }
        match body["next_cursor"].as_str() {
            Some(next) => cursor = Some(next.to_string()),
            None => break,
        }
    }
    assert_eq!(pages, 3);
    assert_eq!(seen, expected);

    // Exactly one full page: no dangling cursor.
    let (_, body) = get(&world, &format!("{base}?limit=5"), Some(&owner.token)).await;
    assert_eq!(body["items"].as_array().unwrap().len(), 5);
    assert_eq!(body["next_cursor"], Value::Null);
    // limit is clamped, not rejected.
    let (status, body) = get(&world, &format!("{base}?limit=0"), Some(&owner.token)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    let (status, _) = get(&world, &format!("{base}?cursor=abc"), Some(&owner.token)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn ended_generations_are_not_listed() {
    let Fixture {
        world,
        owner,
        profile,
    } = fixture().await;
    let peer = RemotePeer::new();
    let leaving = RemotePeer::new();
    for who in [&peer, &leaving] {
        let follow_id = who.activity_id("follow-1");
        let status = who
            .post(
                &world,
                &profile.nanoid,
                &who.follow(&follow_id, &profile.actor_url),
            )
            .await;
        assert_eq!(status, StatusCode::ACCEPTED);
    }
    let undo = leaving.undo(
        &leaving.activity_id("undo-1"),
        serde_json::Value::String(leaving.activity_id("follow-1")),
    );
    assert_eq!(
        leaving.post(&world, &profile.nanoid, &undo).await,
        StatusCode::ACCEPTED
    );

    let (_, body) = get(
        &world,
        &format!("/api/v1/profiles/{}/followers", profile.nanoid),
        Some(&owner.token),
    )
    .await;
    let items = body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["follower"]["actor_url"], peer.actor_url.as_str());
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn only_the_current_owner_reads_and_only_that_profiles_relations() {
    let Fixture {
        world,
        owner,
        profile,
    } = fixture().await;
    let sibling = world.create_profile(&owner, true).await;
    let outsider = world.new_user().await;
    let foreign = world.create_profile(&outsider, true).await;
    let peer = RemotePeer::new();
    world
        .seed_follow(
            remote(&peer),
            local(&profile),
            &peer.activity_id("f1"),
            true,
        )
        .await;
    world
        .seed_follow(
            remote(&peer),
            local(&sibling),
            &peer.activity_id("f2"),
            true,
        )
        .await;
    world
        .seed_follow(
            local(&foreign),
            remote(&peer),
            &format!("{}/follows/1", foreign.actor_url),
            true,
        )
        .await;

    for kind in ["followers", "following"] {
        let uri = |nanoid: &str| format!("/api/v1/profiles/{nanoid}/{kind}");
        // Not authenticated.
        assert_eq!(
            get(&world, &uri(&profile.nanoid), None).await.0,
            StatusCode::UNAUTHORIZED
        );
        // Authenticated, but another user's Profile.
        assert_eq!(
            get(&world, &uri(&profile.nanoid), Some(&outsider.token))
                .await
                .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            get(&world, &uri(&foreign.nanoid), Some(&owner.token))
                .await
                .0,
            StatusCode::FORBIDDEN
        );
        // Unknown Profile.
        assert_eq!(
            get(&world, &uri("no-such-profile"), Some(&owner.token))
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }

    // The owner of two Profiles sees each Profile's own relations only.
    let (_, body) = get(
        &world,
        &format!("/api/v1/profiles/{}/followers", profile.nanoid),
        Some(&owner.token),
    )
    .await;
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["items"][0]["profile_id"], profile.nanoid.as_str());
    let (_, body) = get(
        &world,
        &format!("/api/v1/profiles/{}/following", profile.nanoid),
        Some(&owner.token),
    )
    .await;
    assert!(
        body["items"].as_array().unwrap().is_empty(),
        "another Account's following relation is not visible"
    );
    let (_, body) = get(
        &world,
        &format!("/api/v1/profiles/{}/following", foreign.nanoid),
        Some(&outsider.token),
    )
    .await;
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
}
