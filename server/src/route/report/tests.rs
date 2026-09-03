use super::test_support::{response_json, ReportTestApp, REPORT_TEST_LOCK};
use axum::http::{Method, StatusCode};
use tower::ServiceExt;
use wiremock::MockServer;

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn create_report_then_listed_then_resolved_then_filtered_out() {
    let _guard = REPORT_TEST_LOCK.lock().await;
    let keto = MockServer::start().await;
    let app = ReportTestApp::new(&keto).await;
    app.mock_moderator(&keto, true, 3).await;

    let create = app.json_request(
        Method::POST,
        "/api/v1/reports",
        serde_json::json!({
            "target": app.target_nanoid,
            "category": "spam",
        }),
    );
    let created = app.router.clone().oneshot(create).await.unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let created = response_json(created).await;
    let report_id = created["id"].as_i64().expect("report id");
    assert_eq!(created["category"], "spam");
    assert_eq!(created["status"], "open");

    let list = app.json_request(Method::GET, "/api/v1/admin/reports", serde_json::json!({}));
    let listed = app.router.clone().oneshot(list).await.unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let listed = response_json(listed).await;
    assert_eq!(listed["open_count"], 1);
    assert_eq!(listed["reports"].as_array().expect("reports").len(), 1);
    assert_eq!(listed["reports"][0]["id"], report_id);

    let resolve = app.json_request(
        Method::POST,
        &format!("/api/v1/admin/reports/{report_id}/resolve"),
        serde_json::json!({"close_reason": "moderated target"}),
    );
    let resolved = app.router.clone().oneshot(resolve).await.unwrap();
    assert_eq!(resolved.status(), StatusCode::NO_CONTENT);

    let list = app.json_request(Method::GET, "/api/v1/admin/reports", serde_json::json!({}));
    let listed = app.router.oneshot(list).await.unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let listed = response_json(listed).await;
    assert_eq!(listed["open_count"], 0);
    assert_eq!(listed["reports"], serde_json::json!([]));
    keto.verify().await;
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn create_report_rejects_other_without_comment() {
    let _guard = REPORT_TEST_LOCK.lock().await;
    let keto = MockServer::start().await;
    let app = ReportTestApp::new(&keto).await;
    let request = app.json_request(
        Method::POST,
        "/api/v1/reports",
        serde_json::json!({"target": app.target_nanoid, "category": "other"}),
    );

    let response = app.router.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn create_report_rejects_invalid_category() {
    let _guard = REPORT_TEST_LOCK.lock().await;
    let keto = MockServer::start().await;
    let app = ReportTestApp::new(&keto).await;
    let request = app.json_request(
        Method::POST,
        "/api/v1/reports",
        serde_json::json!({"target": app.target_nanoid, "category": "illegal"}),
    );

    let response = app.router.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn create_report_returns_not_found_for_unknown_target() {
    let _guard = REPORT_TEST_LOCK.lock().await;
    let keto = MockServer::start().await;
    let app = ReportTestApp::new(&keto).await;
    let request = app.json_request(
        Method::POST,
        "/api/v1/reports",
        serde_json::json!({"target": "unknown-target", "category": "spam"}),
    );

    let response = app.router.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn list_reports_rejects_non_moderator() {
    let _guard = REPORT_TEST_LOCK.lock().await;
    let keto = MockServer::start().await;
    let app = ReportTestApp::new(&keto).await;
    app.mock_moderator(&keto, false, 1).await;
    let request = app.json_request(Method::GET, "/api/v1/admin/reports", serde_json::json!({}));

    let response = app.router.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    keto.verify().await;
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn resolve_report_rejects_already_closed_report() {
    let _guard = REPORT_TEST_LOCK.lock().await;
    let keto = MockServer::start().await;
    let app = ReportTestApp::new(&keto).await;
    app.mock_moderator(&keto, true, 2).await;
    let create = app.json_request(
        Method::POST,
        "/api/v1/reports",
        serde_json::json!({"target": app.target_nanoid, "category": "spam"}),
    );
    let created = app.router.clone().oneshot(create).await.unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let report_id = response_json(created).await["id"]
        .as_i64()
        .expect("report id");
    let uri = format!("/api/v1/admin/reports/{report_id}/resolve");
    let first = app.json_request(
        Method::POST,
        &uri,
        serde_json::json!({"close_reason": "moderated target"}),
    );
    assert_eq!(
        app.router.clone().oneshot(first).await.unwrap().status(),
        StatusCode::NO_CONTENT
    );
    let second = app.json_request(
        Method::POST,
        &uri,
        serde_json::json!({"close_reason": "duplicate close"}),
    );

    let response = app.router.oneshot(second).await.unwrap();

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    keto.verify().await;
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn resolve_report_accepts_warned_resolution() {
    let _guard = REPORT_TEST_LOCK.lock().await;
    let keto = MockServer::start().await;
    let app = ReportTestApp::new(&keto).await;
    app.mock_moderator(&keto, true, 1).await;
    let create = app.json_request(
        Method::POST,
        "/api/v1/reports",
        serde_json::json!({"target": app.target_nanoid, "category": "spam"}),
    );
    let created = app.router.clone().oneshot(create).await.unwrap();
    let report_id = response_json(created).await["id"]
        .as_i64()
        .expect("report id");
    let resolve = app.json_request(
        Method::POST,
        &format!("/api/v1/admin/reports/{report_id}/resolve"),
        serde_json::json!({"resolution": "warned", "close_reason": "warning issued"}),
    );

    let response = app.router.oneshot(resolve).await.unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    keto.verify().await;
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn warning_endpoint_issues_and_lists_warning_history() {
    let _guard = REPORT_TEST_LOCK.lock().await;
    let keto = MockServer::start().await;
    let app = ReportTestApp::new(&keto).await;
    app.mock_moderator(&keto, true, 2).await;
    let uri = format!("/api/v1/admin/accounts/{}/warnings", app.target_nanoid);
    let warn = app.json_request(
        Method::POST,
        &uri,
        serde_json::json!({"reason": "be respectful"}),
    );
    let warned = app.router.clone().oneshot(warn).await.unwrap();
    assert_eq!(warned.status(), StatusCode::NO_CONTENT);
    let list = app.json_request(Method::GET, &uri, serde_json::json!({}));

    let response = app.router.oneshot(list).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let warnings = response_json(response).await;
    assert_eq!(warnings.as_array().expect("warnings").len(), 1);
    assert_eq!(warnings[0]["reason"], "be respectful");
    assert!(warnings[0]["warned_at"].is_string());
    keto.verify().await;
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn warning_endpoint_rejects_non_moderator() {
    let _guard = REPORT_TEST_LOCK.lock().await;
    let keto = MockServer::start().await;
    let app = ReportTestApp::new(&keto).await;
    app.mock_moderator(&keto, false, 1).await;
    let request = app.json_request(
        Method::POST,
        &format!("/api/v1/admin/accounts/{}/warnings", app.target_nanoid),
        serde_json::json!({"reason": "be respectful"}),
    );

    let response = app.router.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    keto.verify().await;
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn resolve_report_rejects_missing_or_blank_close_reason() {
    let _guard = REPORT_TEST_LOCK.lock().await;
    let keto = MockServer::start().await;
    let app = ReportTestApp::new(&keto).await;
    for body in [
        serde_json::json!({}),
        serde_json::json!({"close_reason": " "}),
    ] {
        let request = app.json_request(Method::POST, "/api/v1/admin/reports/1/resolve", body);
        let response = app.router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn dismiss_report_closes_open_report() {
    let _guard = REPORT_TEST_LOCK.lock().await;
    let keto = MockServer::start().await;
    let app = ReportTestApp::new(&keto).await;
    app.mock_moderator(&keto, true, 1).await;
    let create = app.json_request(
        Method::POST,
        "/api/v1/reports",
        serde_json::json!({"target": app.target_nanoid, "category": "spam"}),
    );
    let created = app.router.clone().oneshot(create).await.unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let report_id = response_json(created).await["id"]
        .as_i64()
        .expect("report id");
    let dismiss = app.json_request(
        Method::POST,
        &format!("/api/v1/admin/reports/{report_id}/dismiss"),
        serde_json::json!({"close_reason": "insufficient evidence"}),
    );

    let response = app.router.oneshot(dismiss).await.unwrap();

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    keto.verify().await;
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn report_routes_reject_unauthenticated_requests() {
    let _guard = REPORT_TEST_LOCK.lock().await;
    let keto = MockServer::start().await;
    let app = ReportTestApp::new(&keto).await;
    for (method, uri) in [
        (Method::POST, "/api/v1/reports"),
        (Method::GET, "/api/v1/admin/reports"),
    ] {
        let request = ReportTestApp::unauthenticated_request(method, uri);
        let response = app.router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
