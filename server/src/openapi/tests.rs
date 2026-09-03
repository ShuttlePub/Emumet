use super::*;

#[test]
#[ignore]
fn write_openapi_spec_to_file() {
    let json = generate_openapi_json();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("openapi.json");
    std::fs::write(&path, &json).expect("Failed to write openapi.json");
    println!("OpenAPI spec written to {}", path.display());
}

#[test]
fn openapi_spec_matches_committed_file() {
    let generated = generate_openapi_json();
    let parsed: serde_json::Value =
        serde_json::from_str(&generated).expect("Generated spec is not valid JSON");
    assert_eq!(parsed["info"]["title"], "Emumet Account Service API");

    let committed_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("openapi.json");
    let committed = std::fs::read_to_string(&committed_path).unwrap_or_else(|_| {
        panic!(
            "openapi.json not found at {}. Generate with: cargo test -p server write_openapi_spec_to_file -- --ignored",
            committed_path.display()
        )
    });
    assert_eq!(
        committed, generated,
        "openapi.json is out of date. Regenerate with: cargo test -p server write_openapi_spec_to_file -- --ignored"
    );
}

#[test]
fn me_endpoint_contract_is_registered() {
    let spec: serde_json::Value = serde_json::from_str(&generate_openapi_json())
        .expect("generated OpenAPI spec is valid JSON");
    let operation = &spec["paths"]["/api/v1/me"]["get"];

    assert!(operation.is_object(), "GET /api/v1/me must be registered");
    assert_eq!(
        operation["security"],
        serde_json::json!([{"bearer_auth": []}]),
        "GET /api/v1/me must require bearer authentication"
    );
    for status in ["200", "401", "503"] {
        assert!(
            operation["responses"].get(status).is_some(),
            "GET /api/v1/me must document {status}"
        );
    }
    assert!(
        spec["components"]["schemas"].get("MeResponse").is_some(),
        "MeResponse schema must be registered"
    );
}

#[test]
fn follow_management_contract_is_registered() {
    let spec: serde_json::Value = serde_json::from_str(&generate_openapi_json())
        .expect("generated OpenAPI spec is valid JSON");
    for (path, method) in [
        ("/api/v1/accounts/{account_id}/unfollow", "post"),
        ("/api/v1/accounts/{account_id}/followers", "get"),
        ("/api/v1/accounts/{account_id}/following", "get"),
    ] {
        let operation = &spec["paths"][path][method];
        assert!(operation.is_object(), "{method} {path} must be registered");
        assert_eq!(
            operation["security"],
            serde_json::json!([{"bearer_auth": []}])
        );
    }
}

#[test]
fn admin_instance_role_contract_is_registered() {
    let spec: serde_json::Value = serde_json::from_str(&generate_openapi_json())
        .expect("generated OpenAPI spec is valid JSON");
    for method in ["put", "delete"] {
        let operation = &spec["paths"]["/api/v1/admin/accounts/{account_id}/roles/{role}"][method];
        assert!(
            operation.is_object(),
            "{method} /api/v1/admin/accounts/{{account_id}}/roles/{{role}} must be registered"
        );
        assert_eq!(
            operation["security"],
            serde_json::json!([{"bearer_auth": []}]),
            "{method} roles endpoint must require bearer authentication"
        );
        for status in ["204", "400", "403", "404"] {
            assert!(
                operation["responses"].get(status).is_some(),
                "{method} roles endpoint must document {status}"
            );
        }
    }
}

#[test]
fn media_upload_contract_is_registered() {
    let spec: serde_json::Value = serde_json::from_str(&generate_openapi_json())
        .expect("generated OpenAPI spec is valid JSON");
    let operation = &spec["paths"]["/api/v1/images"]["post"];

    assert!(operation.is_object());
    assert_eq!(
        operation["security"],
        serde_json::json!([{"bearer_auth": []}])
    );
    assert_eq!(
        operation["requestBody"]["content"]
            .as_object()
            .map(|content| content.contains_key("multipart/form-data")),
        Some(true)
    );
}

#[test]
fn account_report_contract_is_registered() {
    let spec: serde_json::Value = serde_json::from_str(&generate_openapi_json())
        .expect("generated OpenAPI spec is valid JSON");
    for (path, method) in [
        ("/api/v1/reports", "post"),
        ("/api/v1/admin/reports", "get"),
        ("/api/v1/admin/reports/{id}/resolve", "post"),
        ("/api/v1/admin/reports/{id}/dismiss", "post"),
    ] {
        let operation = &spec["paths"][path][method];
        assert!(operation.is_object(), "{method} {path} must be registered");
        assert_eq!(
            operation["security"],
            serde_json::json!([{"bearer_auth": []}])
        );
    }
    for schema in [
        "CreateReportRequest",
        "CloseReportRequest",
        "AccountReportResponse",
        "AccountReportListResponse",
    ] {
        assert!(
            spec["components"]["schemas"].get(schema).is_some(),
            "{schema} must be registered"
        );
    }
}

#[test]
fn profile_transfer_contract_is_registered() {
    let spec: serde_json::Value = serde_json::from_str(&generate_openapi_json())
        .expect("generated OpenAPI spec is valid JSON");
    for (path, method) in [
        (
            "/api/v1/profiles/{profile_nanoid}/transfer-requests",
            "post",
        ),
        (
            "/api/v1/profile-transfer-requests/{request_nanoid}/accept",
            "post",
        ),
        (
            "/api/v1/profile-transfer-requests/{request_nanoid}/reject",
            "post",
        ),
        (
            "/api/v1/profile-transfer-requests/{request_nanoid}/cancel",
            "post",
        ),
    ] {
        let operation = &spec["paths"][path][method];
        assert!(operation.is_object(), "{method} {path} must be registered");
        assert_eq!(
            operation["security"],
            serde_json::json!([{"bearer_auth": []}]),
            "{method} {path} must require bearer authentication"
        );
    }
    for schema in [
        "CreateProfileTransferRequest",
        "ProfileTransferRequestResponse",
    ] {
        assert!(
            spec["components"]["schemas"].get(schema).is_some(),
            "{schema} must be registered"
        );
    }
}
