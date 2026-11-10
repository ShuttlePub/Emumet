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

fn generated_spec() -> serde_json::Value {
    serde_json::from_str(&generate_openapi_json()).expect("generated OpenAPI spec is valid JSON")
}

#[test]
fn profile_policy_contract_is_registered_with_authorization_and_errors() {
    let spec = generated_spec();
    let profile = "/api/v1/profiles/{profile_nanoid}";
    for (path, method, statuses) in [
        (
            profile.to_string(),
            "get",
            vec!["200", "401", "403", "404", "501"],
        ),
        (
            format!("{profile}/follows"),
            "post",
            vec!["201", "401", "403", "404", "409", "422", "501"],
        ),
        (
            format!("{profile}/follows/{{follow_id}}"),
            "delete",
            vec!["204", "401", "403", "404", "501"],
        ),
        (
            format!("{profile}/following"),
            "get",
            vec!["200", "401", "403", "404", "501"],
        ),
        (
            format!("{profile}/followers"),
            "get",
            vec!["200", "401", "403", "404", "501"],
        ),
        (
            format!("{profile}/blocks"),
            "get",
            vec!["200", "401", "403", "404", "501"],
        ),
        (
            format!("{profile}/blocks"),
            "post",
            vec!["201", "401", "403", "404", "409", "501"],
        ),
        (
            format!("{profile}/blocks/{{block_id}}"),
            "delete",
            vec!["204", "401", "403", "404", "501"],
        ),
        (
            format!("{profile}/mutes"),
            "get",
            vec!["200", "401", "403", "404", "501"],
        ),
        (
            format!("{profile}/mutes"),
            "post",
            vec!["201", "401", "403", "404", "409", "501"],
        ),
        (
            format!("{profile}/mutes/{{mute_id}}"),
            "delete",
            vec!["204", "401", "403", "404", "501"],
        ),
        (
            format!("{profile}/domain-controls"),
            "get",
            vec!["200", "401", "403", "404", "501"],
        ),
        (
            format!("{profile}/domain-controls/{{domain}}"),
            "get",
            vec!["200", "401", "403", "404", "501"],
        ),
        (
            format!("{profile}/domain-controls/{{domain}}"),
            "put",
            vec!["200", "401", "403", "404", "409", "422", "501"],
        ),
        (
            format!("{profile}/domain-controls/{{domain}}"),
            "delete",
            vec!["204", "401", "403", "404", "409", "501"],
        ),
    ] {
        let operation = &spec["paths"][path.as_str()][method];
        assert!(operation.is_object(), "{method} {path} must be registered");
        assert_eq!(
            operation["security"],
            serde_json::json!([{"bearer_auth": []}]),
            "{method} {path} must require bearer authentication"
        );
        for status in statuses {
            assert!(
                operation["responses"].get(status).is_some(),
                "{method} {path} must document {status}"
            );
        }
        let description = operation["description"].as_str().unwrap_or_default();
        assert!(
            description.contains("NOT MOUNTED"),
            "{method} {path} description must state it is not mounted in production"
        );
        assert!(
            description.contains("current owner"),
            "{method} {path} description must state owner-only authorization"
        );
    }

    let actor = &spec["paths"]["/ap/profiles/{profile_nanoid}"]["get"];
    assert!(
        actor.is_object(),
        "Profile actor contract must be registered"
    );
    assert!(actor["description"]
        .as_str()
        .unwrap_or_default()
        .contains("NOT MOUNTED"));
    assert!(actor["responses"].get("501").is_some());
}

#[test]
fn domain_control_contract_defines_revision_and_exact_match_semantics() {
    let spec = generated_spec();
    let schemas = &spec["components"]["schemas"];
    for schema in [
        "DomainControlMode",
        "DomainControlResponse",
        "DomainControlListResponse",
        "PutDomainControlRequest",
    ] {
        assert!(schemas.get(schema).is_some(), "{schema} must be registered");
    }
    assert_eq!(
        schemas["DomainControlMode"]["enum"],
        serde_json::json!(["mute", "block"])
    );
    assert!(schemas["DomainControlResponse"]["required"]
        .as_array()
        .unwrap()
        .iter()
        .any(|field| field == "revision"));
    let put_required = schemas["PutDomainControlRequest"]["required"]
        .as_array()
        .unwrap();
    assert!(put_required.iter().any(|field| field == "mode"));
    assert!(
        !put_required
            .iter()
            .any(|field| field == "expected_revision"),
        "expected_revision is optional (omitted = create)"
    );

    let path = "/api/v1/profiles/{profile_nanoid}/domain-controls/{domain}";
    let delete_params = spec["paths"][path]["delete"]["parameters"]
        .as_array()
        .unwrap();
    let expected_revision = delete_params
        .iter()
        .find(|p| p["name"] == "expected_revision")
        .expect("DELETE must take expected_revision");
    assert_eq!(expected_revision["in"], "query");
    assert_eq!(expected_revision["required"], true);
    assert!(spec["paths"][path]["get"]["description"]
        .as_str()
        .unwrap()
        .contains("exact"));
}

#[test]
fn username_is_optional_nullable_and_has_no_update_api() {
    let spec = generated_spec();
    let schemas = &spec["components"]["schemas"];
    let pattern = "^[a-z0-9][a-z0-9._-]{0,63}$";

    for (schema, field) in [
        ("CreateAccountRequest", "profile_username"),
        ("CreateProfileRequest", "username"),
    ] {
        let required = schemas[schema]["required"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            !required.iter().any(|name| name == field),
            "{schema}.{field} must stay optional until P4"
        );
        assert_eq!(
            schemas[schema]["properties"][field]["pattern"], pattern,
            "{schema}.{field} must publish the username pattern"
        );
    }
    assert!(schemas["CreateAccountRequest"]["required"]
        .as_array()
        .unwrap()
        .iter()
        .any(|name| name == "name"));

    let profile = &schemas["ProfileResponse"];
    assert!(profile["properties"].get("username").is_some());
    assert!(
        !profile["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|name| name == "username"),
        "ProfileResponse.username must be able to represent an unset username"
    );

    for path in ["/api/v1/accounts", "/api/v1/profiles"] {
        assert!(
            spec["paths"][path]["post"]["responses"]
                .get("501")
                .is_some(),
            "POST {path} must document the explicit unimplemented-username error"
        );
        assert!(spec["paths"][path]["post"]["responses"]
            .get("400")
            .is_some());
    }

    let paths = spec["paths"].as_object().unwrap();
    for (path, item) in paths {
        assert!(
            !path.contains("username"),
            "{path}: username must not get its own (update) route"
        );
        if path.starts_with("/api/v1/profiles/{profile_nanoid}") {
            for method in ["put", "patch"] {
                if let Some(operation) = item.get(method) {
                    let body = operation["requestBody"].to_string();
                    assert!(
                        !body.contains("username"),
                        "{method} {path} must not accept a username"
                    );
                }
            }
        }
    }
    assert!(
        spec["paths"]["/api/v1/accounts/{account_id}"]["patch"]["requestBody"]
            .to_string()
            .contains("UpdateAccountRequest")
    );
    assert!(schemas["UpdateAccountRequest"]["properties"]
        .get("username")
        .is_none());
    assert!(schemas["UpdateAccountRequest"]["properties"]
        .get("profile_username")
        .is_none());
}

#[test]
fn contract_only_tag_states_it_is_not_mounted() {
    let spec = generated_spec();
    let tag = spec["tags"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tag| tag["name"] == "ProfilePolicy")
        .expect("ProfilePolicy tag must be registered");
    let description = tag["description"].as_str().unwrap();
    assert!(description.contains("CONTRACT ONLY"));
    assert!(description.contains("not mounted"));
}

#[test]
fn followers_contract_identifies_local_and_remote_followers() {
    let spec = generated_spec();
    let operation = &spec["paths"]["/api/v1/profiles/{profile_nanoid}/followers"]["get"];
    assert_eq!(
        operation["responses"]["200"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/ProfileFollowerListResponse",
        "followers must not reuse the following response type"
    );
    let schemas = &spec["components"]["schemas"];
    assert_eq!(
        schemas["ProfileFollowerListResponse"]["properties"]["items"]["items"]["$ref"],
        "#/components/schemas/ProfileFollowerResponse"
    );
    assert_eq!(
        schemas["ProfileFollowerResponse"]["properties"]["follower"]["$ref"],
        "#/components/schemas/ProfileActorRef"
    );
    let actor = &schemas["ProfileActorRef"];
    let required = actor["required"].as_array().unwrap();
    assert!(required.iter().any(|f| f == "kind"));
    assert!(required.iter().any(|f| f == "actor_url"));
    assert!(
        !required.iter().any(|f| f == "profile_id"),
        "profile_id is only present for local followers"
    );
    assert_eq!(
        schemas["ProfileActorKind"]["enum"],
        serde_json::json!(["local", "remote"])
    );
}

#[test]
fn contract_only_component_schemas_state_they_are_not_mounted() {
    let spec = generated_spec();
    let schemas = spec["components"]["schemas"].as_object().unwrap();
    let contract_schemas = [
        "CreateProfileFollowRequest",
        "ProfileFollowState",
        "ProfileFollowResponse",
        "ProfileFollowListResponse",
        "ProfileActorRef",
        "ProfileActorKind",
        "ProfileFollowerResponse",
        "ProfileFollowerListResponse",
        "CreateProfileRelationRequest",
        "ProfileRelationKind",
        "ProfileRelationResponse",
        "ProfileRelationListResponse",
        "DomainControlMode",
        "DomainControlResponse",
        "DomainControlListResponse",
        "PutDomainControlRequest",
    ];
    for name in contract_schemas {
        let description = schemas
            .get(name)
            .unwrap_or_else(|| panic!("{name} must be registered"))["description"]
            .as_str()
            .unwrap_or_default();
        assert!(
            description.contains("CONTRACT ONLY") && description.contains("not mounted"),
            "{name} description must state the not-mounted boundary, got {description:?}"
        );
    }
    let registered_contract_types = schemas
        .keys()
        .filter(|name| {
            schemas[name.as_str()]["description"]
                .as_str()
                .is_some_and(|d| d.contains("CONTRACT ONLY"))
        })
        .count();
    assert_eq!(
        registered_contract_types,
        contract_schemas.len(),
        "every contract-only schema must be listed in this test"
    );
}
