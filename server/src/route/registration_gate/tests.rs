use super::test_support::{
    observation, request, response_json, GateTestApp, DOWN, GATE_ROUTE_TEST_LOCK, HEALTHY, OFF,
};
use axum::http::{Method, StatusCode};
use tower::ServiceExt;
use wiremock::MockServer;

const BASE: &str = "/api/v1/admin/registration-gate";

fn endpoints() -> Vec<(Method, String, Option<serde_json::Value>)> {
    vec![
        (Method::GET, BASE.to_string(), None),
        (
            Method::POST,
            format!("{BASE}/open"),
            Some(serde_json::json!({"revision": 0})),
        ),
        (
            Method::POST,
            format!("{BASE}/close"),
            Some(serde_json::json!({"revision": 0})),
        ),
        (
            Method::POST,
            format!("{BASE}/override"),
            Some(serde_json::json!({"revision": 0, "acknowledge_warning": true})),
        ),
        (
            Method::POST,
            format!("{BASE}/override/revoke"),
            Some(serde_json::json!({"revision": 0})),
        ),
        (Method::GET, format!("{BASE}/notifications"), None),
        (
            Method::GET,
            format!("{BASE}/notifications/unread-count"),
            None,
        ),
        (
            Method::POST,
            format!("{BASE}/notifications/read"),
            Some(serde_json::json!({"event_ids": ["1"]})),
        ),
        (Method::POST, format!("{BASE}/notifications/read-all"), None),
    ]
}

async fn json_of(
    app: &GateTestApp,
    user: &super::test_support::TestUser,
    method: Method,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let response = app
        .router
        .clone()
        .oneshot(request(Some(user), method, uri, body))
        .await
        .unwrap();
    let status = response.status();
    let body = if status.is_success() {
        response_json(response).await
    } else {
        serde_json::Value::Null
    };
    (status, body)
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn every_endpoint_requires_authentication() {
    let _guard = GATE_ROUTE_TEST_LOCK.lock().await;
    let keto = MockServer::start().await;
    let app = GateTestApp::new(&keto).await;

    for (method, uri, body) in endpoints() {
        let response = app
            .router
            .clone()
            .oneshot(request(None, method.clone(), &uri, body))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {uri}"
        );
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn every_endpoint_rejects_non_admin_and_leaves_the_gate_untouched() {
    let _guard = GATE_ROUTE_TEST_LOCK.lock().await;
    let keto = MockServer::start().await;
    let app = GateTestApp::new(&keto).await;
    app.observe(observation(HEALTHY, OFF)).await;

    for (method, uri, body) in endpoints() {
        let (status, _) = json_of(&app, &app.outsider, method.clone(), &uri, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}");
    }

    let (status, state) = json_of(&app, &app.admin_a, Method::GET, BASE, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(state["revision"], 0);
    assert_eq!(state["admin_state"], "closed");
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn open_close_flow_with_revision_conflict_and_minimum_condition() {
    let _guard = GATE_ROUTE_TEST_LOCK.lock().await;
    let keto = MockServer::start().await;
    let app = GateTestApp::new(&keto).await;

    let (status, initial) = json_of(&app, &app.admin_a, Method::GET, BASE, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(initial["revision"], 0);
    assert_eq!(initial["admin_state"], "closed");
    assert_eq!(initial["decision"], "admin_closed");
    assert_eq!(initial["accepting"], false);
    assert_eq!(initial["override"]["active"], false);

    let open = |revision: i64| Some(serde_json::json!({ "revision": revision }));
    let (status, _) = json_of(
        &app,
        &app.admin_a,
        Method::POST,
        &format!("{BASE}/open"),
        open(0),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "minimum condition unmet"
    );

    app.observe(observation(HEALTHY, OFF)).await;
    let (status, opened) = json_of(
        &app,
        &app.admin_a,
        Method::POST,
        &format!("{BASE}/open"),
        open(0),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(opened["revision"], 1);
    assert_eq!(opened["decision"], "accepting");
    assert_eq!(opened["enforced_protections"], serde_json::json!(["email"]));

    let (status, _) = json_of(
        &app,
        &app.admin_b,
        Method::POST,
        &format!("{BASE}/close"),
        open(0),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "stale revision");

    let (status, closed) = json_of(
        &app,
        &app.admin_b,
        Method::POST,
        &format!("{BASE}/close"),
        open(1),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(closed["decision"], "admin_closed");

    app.observe(observation(HEALTHY, HEALTHY)).await;
    let (_, after_recovery) = json_of(&app, &app.admin_a, Method::GET, BASE, None).await;
    assert_eq!(
        after_recovery["decision"], "admin_closed",
        "manual close is never lifted automatically"
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn override_requires_ack_is_always_visible_and_revoke_reevaluates() {
    let _guard = GATE_ROUTE_TEST_LOCK.lock().await;
    let keto = MockServer::start().await;
    let app = GateTestApp::new(&keto).await;
    let uri = format!("{BASE}/override");

    let (status, _) = json_of(
        &app,
        &app.admin_a,
        Method::POST,
        &uri,
        Some(serde_json::json!({"revision": 0})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "missing warning ack");
    let (status, _) = json_of(
        &app,
        &app.admin_a,
        Method::POST,
        &uri,
        Some(serde_json::json!({"revision": 0, "acknowledge_warning": false})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, enabled) = json_of(
        &app,
        &app.admin_a,
        Method::POST,
        &uri,
        Some(serde_json::json!({"revision": 0, "acknowledge_warning": true})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(enabled["override"]["active"], true);
    assert_eq!(
        enabled["override"]["activated_by"],
        app.admin_a.auth_account_id.as_ref().to_string()
    );
    assert_eq!(enabled["decision"], "accepting");

    app.observe(observation(OFF, OFF)).await;
    let (_, still) = json_of(&app, &app.admin_b, Method::GET, BASE, None).await;
    assert_eq!(still["override"]["active"], true, "visible to every admin");
    assert_eq!(
        still["decision"], "accepting",
        "no auto stop while override"
    );
    assert_eq!(still["config_lost_latch"], false);

    let (status, revoked) = json_of(
        &app,
        &app.admin_b,
        Method::POST,
        &format!("{BASE}/override/revoke"),
        Some(serde_json::json!({"revision": enabled["revision"]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(revoked["override"]["active"], false);
    assert_eq!(revoked["decision"], "stopped");
    assert_eq!(revoked["requires_manual_resume"], true);
    assert_eq!(revoked["admin_state"], "open");

    let (status, _) = json_of(
        &app,
        &app.admin_a,
        Method::POST,
        &format!("{BASE}/open"),
        Some(serde_json::json!({"revision": revoked["revision"]})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "resume needs protection"
    );

    app.observe(observation(HEALTHY, OFF)).await;
    let (_, still_stopped) = json_of(&app, &app.admin_a, Method::GET, BASE, None).await;
    assert_eq!(still_stopped["decision"], "stopped", "no auto resume");
    let (status, resumed) = json_of(
        &app,
        &app.admin_a,
        Method::POST,
        &format!("{BASE}/open"),
        Some(serde_json::json!({"revision": still_stopped["revision"]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(resumed["decision"], "accepting");
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn malformed_bodies_are_bad_requests() {
    let _guard = GATE_ROUTE_TEST_LOCK.lock().await;
    let keto = MockServer::start().await;
    let app = GateTestApp::new(&keto).await;

    for body in [
        serde_json::json!({}),
        serde_json::json!({"revision": "zero"}),
    ] {
        let (status, _) = json_of(
            &app,
            &app.admin_a,
            Method::POST,
            &format!("{BASE}/open"),
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn notifications_list_unread_count_and_read_state_are_per_admin() {
    let _guard = GATE_ROUTE_TEST_LOCK.lock().await;
    let keto = MockServer::start().await;
    let app = GateTestApp::new(&keto).await;
    app.observe(observation(HEALTHY, HEALTHY)).await;
    let (status, _) = json_of(
        &app,
        &app.admin_a,
        Method::POST,
        &format!("{BASE}/open"),
        Some(serde_json::json!({"revision": 0})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    for _ in 0..3 {
        app.observe(observation(DOWN, DOWN)).await;
    }
    app.observe(observation(HEALTHY, HEALTHY)).await;

    let count_uri = format!("{BASE}/notifications/unread-count");
    let (_, a_count) = json_of(&app, &app.admin_a, Method::GET, &count_uri, None).await;
    let (_, b_count) = json_of(&app, &app.admin_b, Method::GET, &count_uri, None).await;
    assert_eq!(
        a_count["unread_count"], 2,
        "repeated probes do not multiply"
    );
    assert_eq!(b_count["unread_count"], 2);

    let list_uri = format!("{BASE}/notifications");
    let (status, listed) = json_of(&app, &app.admin_a, Method::GET, &list_uri, None).await;
    assert_eq!(status, StatusCode::OK);
    let events = listed["notifications"].as_array().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["kind"], "outage_recovered");
    assert_eq!(events[1]["kind"], "outage_started");
    assert!(events.iter().all(|e| e["read"] == false));
    assert!(events[0]["id"].is_string());

    let newest = events[0]["id"].as_str().unwrap().to_string();
    let (status, after_read) = json_of(
        &app,
        &app.admin_a,
        Method::POST,
        &format!("{BASE}/notifications/read"),
        Some(serde_json::json!({"event_ids": [newest]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(after_read["unread_count"], 1);

    let (_, b_after) = json_of(&app, &app.admin_b, Method::GET, &count_uri, None).await;
    assert_eq!(b_after["unread_count"], 2, "B is unaffected by A's read");
    let (_, b_list) = json_of(&app, &app.admin_b, Method::GET, &list_uri, None).await;
    assert!(b_list["notifications"]
        .as_array()
        .unwrap()
        .iter()
        .all(|e| e["read"] == false));
    let (_, a_list) = json_of(&app, &app.admin_a, Method::GET, &list_uri, None).await;
    assert_eq!(a_list["notifications"][0]["read"], true);
    assert_eq!(a_list["notifications"][1]["read"], false);

    let (status, all_read) = json_of(
        &app,
        &app.admin_b,
        Method::POST,
        &format!("{BASE}/notifications/read-all"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(all_read["unread_count"], 0);
    let (_, a_final) = json_of(&app, &app.admin_a, Method::GET, &count_uri, None).await;
    assert_eq!(
        a_final["unread_count"], 1,
        "A is unaffected by B's read-all"
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn notifications_support_paging_and_reject_invalid_ids() {
    let _guard = GATE_ROUTE_TEST_LOCK.lock().await;
    let keto = MockServer::start().await;
    let app = GateTestApp::new(&keto).await;
    app.observe(observation(HEALTHY, HEALTHY)).await;
    let (status, _) = json_of(
        &app,
        &app.admin_a,
        Method::POST,
        &format!("{BASE}/open"),
        Some(serde_json::json!({"revision": 0})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    app.observe(observation(DOWN, DOWN)).await;
    app.observe(observation(HEALTHY, HEALTHY)).await;

    let (status, first) = json_of(
        &app,
        &app.admin_a,
        Method::GET,
        &format!("{BASE}/notifications?limit=1"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first["notifications"].as_array().unwrap().len(), 1);
    let cursor = first["next_cursor"].as_str().expect("next cursor");
    let (_, second) = json_of(
        &app,
        &app.admin_a,
        Method::GET,
        &format!("{BASE}/notifications?limit=1&before={cursor}"),
        None,
    )
    .await;
    assert_eq!(second["notifications"][0]["kind"], "outage_started");

    let (status, _) = json_of(
        &app,
        &app.admin_a,
        Method::GET,
        &format!("{BASE}/notifications?before=abc"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    for body in [
        serde_json::json!({"event_ids": []}),
        serde_json::json!({"event_ids": ["not-a-number"]}),
    ] {
        let (status, _) = json_of(
            &app,
            &app.admin_a,
            Method::POST,
            &format!("{BASE}/notifications/read"),
            Some(body),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}

#[test]
fn openapi_documents_admin_only_registration_gate_contract() {
    let spec: serde_json::Value =
        serde_json::from_str(&crate::openapi::generate_openapi_json()).expect("valid OpenAPI");
    for (method, path, statuses) in [
        ("get", "", vec!["200", "401", "403"]),
        ("post", "/open", vec!["200", "401", "403", "409", "422"]),
        ("post", "/close", vec!["200", "401", "403", "409", "422"]),
        (
            "post",
            "/override",
            vec!["200", "400", "401", "403", "409", "422"],
        ),
        (
            "post",
            "/override/revoke",
            vec!["200", "401", "403", "409", "422"],
        ),
        ("get", "/notifications", vec!["200", "401", "403"]),
        (
            "get",
            "/notifications/unread-count",
            vec!["200", "401", "403"],
        ),
        (
            "post",
            "/notifications/read",
            vec!["200", "400", "401", "403"],
        ),
        ("post", "/notifications/read-all", vec!["200", "401", "403"]),
    ] {
        let operation = &spec["paths"][format!("{BASE}{path}")][method];
        assert!(operation.is_object(), "{method} {BASE}{path} registered");
        assert_eq!(
            operation["security"],
            serde_json::json!([{"bearer_auth": []}])
        );
        for status in statuses {
            assert!(
                operation["responses"].get(status).is_some(),
                "{method} {BASE}{path} documents {status}"
            );
        }
    }
    assert!(
        spec["components"]["schemas"]["RegistrationGateResponse"]["properties"]["override"]
            .is_object()
    );
}
