use super::build_test_router;
use crate::handler::AppModule;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

const HTTP_METHODS: &[&str] = &[
    "get", "post", "put", "delete", "patch", "options", "head", "trace",
];

async fn app() -> axum::Router {
    let module = AppModule::new_for_oauth2_test(
        "http://localhost:65535".into(),
        "http://localhost:65535".into(),
    )
    .await
    .expect("AppModule init failed (is DATABASE_URL set?)");
    build_test_router(module)
}

struct RouteCase {
    method: String,
    uri: String,
    requires_auth: bool,
    allows_not_found: bool,
    accept: Option<&'static str>,
}

fn replace_path_params(template: &str) -> String {
    let mut result = String::with_capacity(template.len());
    let mut chars = template.chars();
    while let Some(c) = chars.next() {
        if c == '{' {
            let param_name: String = chars.by_ref().take_while(|&ch| ch != '}').collect();
            result.push_str(&format!("test-{param_name}"));
        } else {
            result.push(c);
        }
    }
    result
}

fn is_authed(security: Option<&serde_json::Value>) -> bool {
    match security {
        Some(arr) => arr.as_array().is_some_and(|a| !a.is_empty()),
        None => false,
    }
}

fn has_not_found_response(operation: &serde_json::Value) -> bool {
    operation
        .get("responses")
        .and_then(|responses| responses.as_object())
        .is_some_and(|responses| responses.contains_key("404"))
}

fn is_activitypub_operation(operation: &serde_json::Value) -> bool {
    operation
        .get("tags")
        .and_then(|tags| tags.as_array())
        .into_iter()
        .flatten()
        .any(|tag| tag.as_str() == Some("ProfileActor"))
}

fn load_routes_from_openapi() -> Vec<RouteCase> {
    let spec_json = crate::openapi::generate_openapi_json();
    let spec: serde_json::Value =
        serde_json::from_str(&spec_json).expect("Failed to parse generated OpenAPI spec");
    let paths = spec["paths"].as_object().expect("paths must be an object");
    let mut cases = Vec::new();

    for (path_template, path_item) in paths {
        let path_item = path_item.as_object().expect("path item must be an object");
        for (key, operation) in path_item {
            if !HTTP_METHODS.contains(&key.as_str()) {
                continue;
            }
            let requires_auth = is_authed(operation.get("security"));
            let concrete_path = replace_path_params(path_template);
            let query_params: Vec<String> = operation
                .get("parameters")
                .and_then(|p| p.as_array())
                .into_iter()
                .flatten()
                .filter(|p| p["in"] == "query" && p["required"] == true)
                .map(|p| format!("{}=smoke", p["name"].as_str().unwrap_or("unknown")))
                .collect();
            let uri = if query_params.is_empty() {
                concrete_path
            } else {
                format!("{}?{}", concrete_path, query_params.join("&"))
            };
            cases.push(RouteCase {
                method: key.to_uppercase(),
                uri,
                requires_auth,
                allows_not_found: !requires_auth && has_not_found_response(operation),
                accept: is_activitypub_operation(operation).then_some("application/activity+json"),
            });
        }
    }
    cases
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn all_openapi_routes_are_reachable() {
    let cases = load_routes_from_openapi();
    assert!(!cases.is_empty(), "No routes found in OpenAPI spec");
    let router = app().await;

    for case in &cases {
        let mut request = Request::builder()
            .method(case.method.as_str())
            .uri(&case.uri)
            .header("content-type", "application/json");
        if let Some(accept) = case.accept {
            request = request.header("accept", accept);
        }
        let request = request.body(Body::from("{}")).unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        let status = response.status();

        if !case.allows_not_found {
            assert_ne!(
                status,
                StatusCode::NOT_FOUND,
                "Route {} {} returned 404 — is it registered?",
                case.method,
                case.uri,
            );
        }
        assert_ne!(
            status,
            StatusCode::METHOD_NOT_ALLOWED,
            "Route {} {} returned 405 — method mismatch?",
            case.method,
            case.uri,
        );
        if case.requires_auth {
            assert_eq!(
                status,
                StatusCode::UNAUTHORIZED,
                "Authed route {} {} expected 401 without token, got {}",
                case.method,
                case.uri,
                status,
            );
        } else {
            assert!(
                status != StatusCode::UNAUTHORIZED && status != StatusCode::FORBIDDEN,
                "Public route {} {} returned {} — accidentally wrapped with auth middleware?",
                case.method,
                case.uri,
                status,
            );
            assert!(
                !status.is_server_error() || status == StatusCode::BAD_GATEWAY,
                "Public route {} {} returned {} — handler or wiring broken?",
                case.method,
                case.uri,
                status,
            );
        }
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn nonexistent_route_returns_404() {
    let router = app().await;
    let request = Request::builder()
        .method("GET")
        .uri("/this/route/does/not/exist")
        .body(Body::empty())
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// The production root router (built by the same function the server binary uses) mounts the
/// domain-controls routes (D8R enables them after the P4 cutover): every documented operation is
/// reachable, authenticated, owner-only and stores / reads the control of the real Profile.
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn domain_controls_are_mounted_on_the_root_router_with_owner_only_access() {
    use crate::route::profile_graph_test_support::GraphWorld;

    let world = GraphWorld::new().await;
    let owner = world.new_user().await;
    let stranger = world.new_user().await;
    let profile = world.create_profile(&owner, true).await;
    let router = world.root_router();
    let nanoid = &profile.nanoid;
    let send = |method: &'static str, uri: String, token: Option<&str>, body: &'static str| {
        let router = router.clone();
        let token = token.map(str::to_string);
        async move {
            let mut builder = Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json");
            if let Some(token) = token {
                builder = builder.header("authorization", format!("Bearer {token}"));
            }
            let response = router
                .oneshot(builder.body(Body::from(body)).unwrap())
                .await
                .unwrap();
            let status = response.status();
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            (
                status,
                serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or_default(),
            )
        }
    };

    // Every documented domain-controls operation is registered (the list comes from OpenAPI).
    let spec: serde_json::Value = serde_json::from_str(&crate::openapi::generate_openapi_json())
        .expect("generated OpenAPI spec");
    let mut documented = 0;
    for (template, item) in spec["paths"].as_object().unwrap() {
        if !template.contains("/domain-controls") {
            continue;
        }
        for method in item.as_object().unwrap().keys() {
            if !HTTP_METHODS.contains(&method.as_str()) {
                continue;
            }
            documented += 1;
            let uri = template
                .replace("{profile_nanoid}", nanoid)
                .replace("{domain}", "example.com");
            let method: &'static str = match method.as_str() {
                "get" => "GET",
                "put" => "PUT",
                "delete" => "DELETE",
                other => panic!("unexpected documented method {other}"),
            };
            let (status, _) = send(method, uri.clone(), None, "{}").await;
            assert_eq!(
                status,
                StatusCode::UNAUTHORIZED,
                "{method} {uri} without a token"
            );
        }
    }
    assert_eq!(
        documented, 4,
        "list, get, put and delete are documented and mounted"
    );

    let list = format!("/api/v1/profiles/{nanoid}/domain-controls");
    let item = format!("{list}/example.com");
    let owner_token = owner.token.as_str();
    let stranger_token = stranger.token.as_str();

    let (status, body) = send("GET", list.clone(), Some(owner_token), "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"], serde_json::json!([]));
    for (method, uri) in [("GET", &list), ("GET", &item), ("PUT", &item)] {
        let (status, _) = send(
            method,
            uri.clone(),
            Some(stranger_token),
            r#"{"mode":"block"}"#,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{method} {uri} by a non-owner"
        );
    }

    let (status, body) = send(
        "PUT",
        item.clone(),
        Some(owner_token),
        r#"{"mode":"block"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        (body["mode"].as_str(), body["revision"].as_u64()),
        (Some("block"), Some(1))
    );
    let (status, _) = send("PUT", item.clone(), Some(owner_token), r#"{"mode":"mute"}"#).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "create on an existing control"
    );
    let (status, _) = send(
        "PUT",
        format!("{list}/bad%2Fdomain"),
        Some(owner_token),
        r#"{"mode":"block"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = send(
        "GET",
        format!("{list}/missing.example"),
        Some(owner_token),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send("DELETE", item.clone(), Some(owner_token), "").await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "expected_revision is required"
    );
    let (status, _) = send(
        "DELETE",
        format!("{item}?expected_revision=9"),
        Some(owner_token),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = send(
        "DELETE",
        format!("{item}?expected_revision=1"),
        Some(owner_token),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send("GET", item, Some(owner_token), "").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "a deleted control is gone");
}

/// A block written through the production root router detaches the stored relations with that
/// exact domain (both directions, pending included) in the same request; an unblock restores
/// nothing, and relations with other domains, other Profiles and other Accounts are untouched.
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_block_through_the_root_router_detaches_relations_and_the_unblock_restores_nothing() {
    use crate::route::profile_graph_test_support::GraphWorld;
    use kernel::prelude::entity::{GraphActorUrl, ProfileGraphActor};

    let world = GraphWorld::new().await;
    let owner = world.new_user().await;
    let profile = world.create_profile(&owner, true).await;
    let other_owner = world.new_user().await;
    let other = world.create_profile(&other_owner, true).await;
    let router = world.root_router();
    let remote = |url: &str| ProfileGraphActor::Remote(GraphActorUrl::new(url).unwrap());
    let me = ProfileGraphActor::Local(profile.id.clone());
    let theirs = ProfileGraphActor::Local(other.id.clone());

    // Mine: following (accepted), following (pending), a follower; other host: untouched.
    world
        .seed_follow(
            me.clone(),
            remote("https://blocked.example/users/a"),
            "https://local.test/f/1",
            true,
        )
        .await;
    world
        .seed_follow(
            me.clone(),
            remote("https://blocked.example/users/b"),
            "https://local.test/f/2",
            false,
        )
        .await;
    world
        .seed_follow(
            remote("https://blocked.example/users/c"),
            me.clone(),
            "https://blocked.example/f/3",
            true,
        )
        .await;
    world
        .seed_follow(
            me.clone(),
            remote("https://sub.blocked.example/users/d"),
            "https://local.test/f/4",
            true,
        )
        .await;
    world
        .seed_follow(
            me.clone(),
            remote("https://elsewhere.example/users/e"),
            "https://local.test/f/5",
            true,
        )
        .await;
    world
        .seed_follow(
            theirs.clone(),
            remote("https://blocked.example/users/a"),
            "https://local.test/f/6",
            true,
        )
        .await;
    let active = |profile_id: i64| {
        let pool = world.pool().clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM profile_follows WHERE state <> 'ended' AND (follower_profile_id = $1 OR followee_profile_id = $1)",
            )
            .bind(profile_id)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(active(*profile.id.as_ref()).await, 5);

    let send = |method: &'static str, uri: String, body: &'static str| {
        let router = router.clone();
        let token = owner.token.clone();
        async move {
            let response = router
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(uri)
                        .header("content-type", "application/json")
                        .header("authorization", format!("Bearer {token}"))
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();
            response.status()
        }
    };
    let item = format!(
        "/api/v1/profiles/{}/domain-controls/blocked.example",
        profile.nanoid
    );

    // A mute detaches nothing.
    assert_eq!(
        send("PUT", item.clone(), r#"{"mode":"mute"}"#).await,
        StatusCode::OK
    );
    assert_eq!(active(*profile.id.as_ref()).await, 5);
    // The block (a replace of the mute) detaches the three relations of exactly that host.
    assert_eq!(
        send(
            "PUT",
            item.clone(),
            r#"{"mode":"block","expected_revision":1}"#
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(
        active(*profile.id.as_ref()).await,
        2,
        "sub.blocked.example and elsewhere.example stay"
    );
    assert_eq!(
        active(*other.id.as_ref()).await,
        1,
        "another Profile is untouched"
    );
    let ended_by_block: i64 =
        sqlx::query_scalar("SELECT count(*) FROM profile_follows WHERE end_reason = 'blocked'")
            .fetch_one(world.pool())
            .await
            .unwrap();
    assert_eq!(ended_by_block, 3);

    // The unblock restores nothing and reserves nothing.
    let jobs_before: i64 = sqlx::query_scalar("SELECT count(*) FROM profile_delivery_jobs")
        .fetch_one(world.pool())
        .await
        .unwrap();
    assert_eq!(
        send("DELETE", format!("{item}?expected_revision=2"), "").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(active(*profile.id.as_ref()).await, 2);
    let jobs_after: i64 = sqlx::query_scalar("SELECT count(*) FROM profile_delivery_jobs")
        .fetch_one(world.pool())
        .await
        .unwrap();
    assert_eq!(jobs_before, jobs_after);
}

/// The retired Account based federation surface answers an explicit `410 Gone` on the production
/// root router: no redirect to the new actor, no signing, no Account scoped Follow / block / mute.
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn retired_account_federation_answers_410_on_the_root_router() {
    use crate::route::profile_graph_test_support::GraphWorld;

    let world = GraphWorld::new().await;
    let owner = world.new_user().await;
    let profile = world.create_profile(&owner, true).await;
    let router = world.root_router();
    let send = |method: &'static str, uri: String, token: Option<String>, accept: bool| {
        let router = router.clone();
        async move {
            let mut builder = Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json");
            if accept {
                builder = builder.header("accept", "application/activity+json");
            }
            if let Some(token) = token {
                builder = builder.header("authorization", format!("Bearer {token}"));
            }
            router
                .oneshot(builder.body(Body::from("{}")).unwrap())
                .await
                .unwrap()
        }
    };

    // /ap/accounts/... (public): 410, never a redirect to the Profile actor.
    for (method, uri) in [
        ("GET", "/ap/accounts"),
        ("GET", "/ap/accounts/some-account"),
        ("POST", "/ap/accounts/some-account/inbox"),
        ("GET", "/ap/accounts/some-account/outbox"),
        ("GET", "/ap/accounts/some-account/followers"),
        ("GET", "/ap/accounts/some-account/following"),
    ] {
        let response = send(method, uri.to_string(), None, true).await;
        assert_eq!(response.status(), StatusCode::GONE, "{method} {uri}");
        assert!(
            response.headers().get("location").is_none(),
            "{method} {uri} must not redirect to the new actor"
        );
    }

    // The signing / public-key endpoints: 410 even for a valid token; 401 without one.
    for (method, uri) in [
        ("POST", "/internal/v1/accounts/some-account/sign"),
        ("GET", "/internal/v1/accounts/some-account/public-key"),
    ] {
        let response = send(method, uri.to_string(), Some(owner.token.clone()), false).await;
        assert_eq!(
            response.status(),
            StatusCode::GONE,
            "{method} {uri} must be retired for an authenticated caller"
        );
        let response = send(method, uri.to_string(), None, false).await;
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {uri}"
        );
    }

    // Account scoped Follow / block / mute operations are retired; the Account management API is not.
    for (method, suffix) in [
        ("POST", "follow"),
        ("POST", "unfollow"),
        ("GET", "followers"),
        ("GET", "following"),
        ("POST", "block"),
        ("POST", "unblock"),
        ("GET", "blocks"),
        ("POST", "mute"),
        ("POST", "unmute"),
        ("GET", "mutes"),
    ] {
        let uri = format!("/api/v1/accounts/some-account/{suffix}");
        let response = send(method, uri.clone(), Some(owner.token.clone()), false).await;
        assert_eq!(response.status(), StatusCode::GONE, "{method} {uri}");
        let response = send(method, uri.clone(), None, false).await;
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {uri}"
        );
    }
    let response = send(
        "GET",
        "/api/v1/accounts/some-account".to_string(),
        Some(owner.token.clone()),
        false,
    )
    .await;
    assert_ne!(
        response.status(),
        StatusCode::GONE,
        "the Account management API stays"
    );

    // The Account WebFinger is gone: the Account name does not resolve, the Profile does.
    let account_name: String = sqlx::query_scalar(
        "SELECT a.name FROM accounts a JOIN profiles p ON p.account_id = a.id WHERE p.id = $1",
    )
    .bind(profile.id.as_ref())
    .fetch_one(world.pool())
    .await
    .unwrap();
    let response = send(
        "GET",
        format!(
            "/.well-known/webfinger?resource=acct:{account_name}@{}",
            world.host_header
        ),
        None,
        false,
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "an Account name must not resolve over WebFinger"
    );
}

/// Positive checks on the production root router with a real Profile and owner: the Profile actor
/// document, WebFinger, the inbox, the identity read, Follow / unfollow, individual block / mute
/// (create / list / delete) and the followers / following reads are all served, with
/// owner-only authorization.
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn profile_federation_routes_are_served_by_the_root_router() {
    use crate::route::profile_graph_test_support::GraphWorld;

    let world = GraphWorld::new().await;
    let owner = world.new_user().await;
    let stranger = world.new_user().await;
    let profile = world.create_profile(&owner, true).await;
    let other_owner = world.new_user().await;
    let other = world.create_profile(&other_owner, true).await;
    let router = world.root_router();
    let nanoid = profile.nanoid.clone();

    let call = |method: &'static str,
                uri: String,
                token: Option<String>,
                body: Option<serde_json::Value>| {
        let router = router.clone();
        let host = world.host_header.clone();
        async move {
            let mut builder = Request::builder()
                .method(method)
                .uri(uri)
                .header("host", host)
                .header("content-type", "application/json");
            if let Some(token) = token {
                builder = builder.header("authorization", format!("Bearer {token}"));
            }
            let body = body.map(|value| value.to_string()).unwrap_or_default();
            let response = router
                .oneshot(builder.body(Body::from(body)).unwrap())
                .await
                .unwrap();
            let status = response.status();
            let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
                .await
                .unwrap();
            let json: serde_json::Value =
                serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
            (status, json)
        }
    };
    let owner_token = Some(owner.token.clone());

    // Actor document, WebFinger (public).
    let (status, actor) = call("GET", format!("/ap/profiles/{nanoid}"), None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(actor["id"], profile.actor_url.as_str());
    assert!(actor["preferredUsername"].is_string());
    assert!(actor["publicKey"]["publicKeyPem"]
        .as_str()
        .unwrap()
        .contains("BEGIN PUBLIC KEY"));
    let username = actor["preferredUsername"].as_str().unwrap().to_string();
    let (status, jrd) = call(
        "GET",
        format!(
            "/.well-known/webfinger?resource=acct:{username}@{}",
            world.host_header
        ),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(jrd["links"][0]["href"], profile.actor_url.as_str());

    // Inbox: mounted, and an unsigned request is refused by the signature check (not 404).
    let (status, _) = call("POST", format!("/ap/profiles/{nanoid}/inbox"), None, None).await;
    assert!(
        status == StatusCode::UNAUTHORIZED || status == StatusCode::BAD_REQUEST,
        "inbox must be mounted and guarded, got {status}"
    );

    // Identity read: owner only.
    let (status, identity) = call(
        "GET",
        format!("/api/v1/profiles/{nanoid}"),
        owner_token.clone(),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(identity["id"], nanoid.as_str());
    assert_eq!(identity["username"], username.as_str());
    let (status, _) = call(
        "GET",
        format!("/api/v1/profiles/{nanoid}"),
        Some(stranger.token.clone()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call("GET", format!("/api/v1/profiles/{nanoid}"), None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Follow a local Profile, list, unfollow.
    let (status, follow) = call(
        "POST",
        format!("/api/v1/profiles/{nanoid}/follows"),
        owner_token.clone(),
        Some(serde_json::json!({"target": other.actor_url})),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{follow}");
    let follow_id = follow["id"].as_str().unwrap().to_string();
    let (status, list) = call(
        "GET",
        format!("/api/v1/profiles/{nanoid}/following"),
        owner_token.clone(),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    let (status, list) = call(
        "GET",
        format!("/api/v1/profiles/{}/followers", other.nanoid),
        Some(other_owner.token.clone()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    let (status, _) = call(
        "DELETE",
        format!("/api/v1/profiles/{nanoid}/follows/{follow_id}"),
        owner_token.clone(),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Individual mute and block: create, list, delete.
    for (kind, plural) in [("mute", "mutes"), ("block", "blocks")] {
        let (status, created) = call(
            "POST",
            format!("/api/v1/profiles/{nanoid}/{plural}"),
            owner_token.clone(),
            Some(serde_json::json!({"target": other.actor_url})),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{kind}: {created}");
        assert_eq!(created["kind"], kind);
        let id = created["id"].as_str().unwrap().to_string();
        let (status, list) = call(
            "GET",
            format!("/api/v1/profiles/{nanoid}/{plural}"),
            owner_token.clone(),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let items = list["items"].as_array().unwrap();
        assert_eq!(items.len(), 1, "{kind}");
        assert_eq!(items[0]["id"], id.as_str());
        assert_eq!(items[0]["target"], other.actor_url.as_str());
        let (status, _) = call(
            "GET",
            format!("/api/v1/profiles/{nanoid}/{plural}"),
            Some(stranger.token.clone()),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{kind} list is owner-only");
        let (status, _) = call(
            "DELETE",
            format!("/api/v1/profiles/{nanoid}/{plural}/{id}"),
            owner_token.clone(),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{kind}");
        let (_, list) = call(
            "GET",
            format!("/api/v1/profiles/{nanoid}/{plural}"),
            owner_token.clone(),
            None,
        )
        .await;
        assert!(list["items"].as_array().unwrap().is_empty(), "{kind}");
    }
}

/// Production serves exactly what `build_root_router` builds (the router the absence test above
/// probes): `main` must not add routes of its own on top of it.
#[test]
fn main_serves_only_the_root_router() {
    let main_rs = include_str!("../main.rs");
    assert!(main_rs.contains("root_router::build_root_router("));
    for forbidden in [".route(", ".nest(", ".merge(", "Router::new"] {
        assert!(
            !main_rs.contains(forbidden),
            "main.rs must not build routes itself ({forbidden})"
        );
    }
}

/// The production entrypoint starts projection, Registration Gate webhook, provider maintenance,
/// independent enforcement probing and (since the P4 cutover) the Profile delivery worker. Whoever
/// adds another worker has to update this list deliberately.
#[test]
fn main_starts_the_expected_workers() {
    let main_rs = include_str!("../main.rs");
    let spawns: Vec<&str> = main_rs
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.starts_with("//") && !line.starts_with("use ") && line.contains("spawn")
        })
        .filter(|line| !line.ends_with("spawn_profile_delivery_worker,"))
        .collect();
    assert_eq!(
        spawns.len(),
        5,
        "unexpected worker start in main.rs: {spawns:?}"
    );
    assert!(spawns[0].contains("ProjectionWorker::spawn"), "{spawns:?}");
    assert!(spawns[1].contains("webhook_worker::spawn("), "{spawns:?}");
    assert!(
        spawns[2].contains("registration_guard::spawn_if_enabled"),
        "{spawns:?}"
    );
    assert!(
        spawns[3].contains("registration_guard::spawn_monitor"),
        "{spawns:?}"
    );
    assert!(
        spawns[4].contains("spawn_profile_delivery_worker("),
        "{spawns:?}"
    );
    assert!(
        !main_rs.contains("tokio::spawn"),
        "main.rs must not spawn tasks itself"
    );
}

/// The Profile actor keys are provisioned right after the migrations and before anything serves
/// requests or delivers: a Profile without a key is not an actor.
#[test]
fn main_provisions_profile_keys_before_workers_and_routes() {
    let main_rs = include_str!("../main.rs");
    let position = |needle: &str| {
        main_rs
            .find(needle)
            .unwrap_or_else(|| panic!("main.rs must contain {needle}"))
    };
    let provision = position("cutover::provision_profile_keys(&app)");
    assert!(position("AppModule::new()") < provision);
    assert!(provision < position("ProjectionWorker::spawn"));
    assert!(provision < position("spawn_profile_delivery_worker("));
    assert!(provision < position("root_router::build_root_router("));
}
