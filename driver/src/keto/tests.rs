use super::*;
use wiremock::matchers::{method, path, query_param, query_param_is_missing};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn new_subject() -> AuthAccountId {
    kernel::ensure_generator_initialized();
    AuthAccountId::default()
}

fn subject_id_string(subject: &AuthAccountId) -> String {
    AsRef::<i64>::as_ref(subject).to_string()
}

fn keto_client(read_url: &str) -> KetoClient {
    KetoClient::new(
        read_url.to_string(),
        "http://unused-write.invalid".to_string(),
    )
}

fn keto_writer(write_url: &str) -> KetoClient {
    KetoClient::new(
        "http://unused-read.invalid".to_string(),
        write_url.to_string(),
    )
}

fn account_target() -> RelationTarget {
    RelationTarget::Account {
        account_id: kernel::prelude::entity::AccountId::default(),
        relation: kernel::interfaces::permission::AccountRelation::Owner,
    }
}

#[tokio::test]
async fn create_relation_is_idempotent_when_tuple_already_exists() {
    // Given
    let server = MockServer::start().await;
    let subject = new_subject();
    Mock::given(method("PUT"))
        .and(path("/admin/relation-tuples"))
        .respond_with(ResponseTemplate::new(409))
        .expect(1)
        .mount(&server)
        .await;

    // When
    let result = keto_writer(&server.uri())
        .create_relation(&account_target(), &subject)
        .await;

    // Then
    assert!(result.is_ok());
    server.verify().await;
}

#[tokio::test]
async fn delete_relation_is_idempotent_when_tuple_is_absent() {
    // Given
    let server = MockServer::start().await;
    let subject = new_subject();
    Mock::given(method("DELETE"))
        .and(path("/admin/relation-tuples"))
        .respond_with(ResponseTemplate::new(404))
        .expect(1)
        .mount(&server)
        .await;

    // When
    let result = keto_writer(&server.uri())
        .delete_relation(&account_target(), &subject)
        .await;

    // Then
    assert!(result.is_ok());
    server.verify().await;
}

/// Given: Keto Read API が admins tuple を返す
/// When: list_instance_roles を呼ぶ
/// Then: GET {read_url}/relation-tuples に namespace=Instance&object=singleton&subject_id=<id> が送られ [Admin] が返る
#[tokio::test]
async fn list_instance_roles_requests_relation_tuples_with_instance_query() {
    let server = MockServer::start().await;
    let subject = new_subject();
    let subject_id = subject_id_string(&subject);

    Mock::given(method("GET"))
        .and(path("/relation-tuples"))
        .and(query_param("namespace", "Instance"))
        .and(query_param("object", "singleton"))
        .and(query_param("subject_id", subject_id.as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "relation_tuples": [{
                "namespace": "Instance",
                "object": "singleton",
                "relation": "admins",
                "subject_id": subject_id
            }],
            "next_page_token": ""
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = keto_client(&server.uri());
    let roles = client.list_instance_roles(&subject).await.unwrap();

    assert_eq!(roles, vec![InstanceRole::Admin]);
    server.verify().await;
}

/// Given: レスポンスが moderators → admins の順で tuple を返す
/// When: list_instance_roles を呼ぶ
/// Then: InstanceRole 宣言順 (Admin → Moderator) の決定的順序で両方返る
#[tokio::test]
async fn list_instance_roles_maps_both_roles_in_declaration_order() {
    let server = MockServer::start().await;
    let subject = new_subject();
    let subject_id = subject_id_string(&subject);

    Mock::given(method("GET"))
        .and(path("/relation-tuples"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "relation_tuples": [
                {"namespace": "Instance", "object": "singleton", "relation": "moderators", "subject_id": subject_id},
                {"namespace": "Instance", "object": "singleton", "relation": "admins", "subject_id": subject_id}
            ],
            "next_page_token": ""
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = keto_client(&server.uri());
    let roles = client.list_instance_roles(&subject).await.unwrap();

    assert_eq!(roles, vec![InstanceRole::Admin, InstanceRole::Moderator]);
    server.verify().await;
}

/// Given: 無関係な relation / 別 namespace / 別 object の tuple が混ざったレスポンス
/// When: list_instance_roles を呼ぶ
/// Then: Instance/singleton の admins / moderators のみが結果に含まれる
#[tokio::test]
async fn list_instance_roles_ignores_unrelated_tuples() {
    let server = MockServer::start().await;
    let subject = new_subject();
    let subject_id = subject_id_string(&subject);

    Mock::given(method("GET"))
        .and(path("/relation-tuples"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "relation_tuples": [
                {"namespace": "Instance", "object": "singleton", "relation": "owners", "subject_id": subject_id},
                {"namespace": "Account", "object": "123", "relation": "admins", "subject_id": subject_id},
                {"namespace": "Instance", "object": "other-object", "relation": "moderators", "subject_id": subject_id},
                {"namespace": "Instance", "object": "singleton", "relation": "moderators", "subject_id": subject_id}
            ],
            "next_page_token": ""
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = keto_client(&server.uri());
    let roles = client.list_instance_roles(&subject).await.unwrap();

    assert_eq!(roles, vec![InstanceRole::Moderator]);
    server.verify().await;
}

/// Given: 1ページ目が非空の next_page_token を返す
/// When: list_instance_roles を呼ぶ
/// Then: page_token 付きで2ページ目も取得し、全ページの tuple を集約する
#[tokio::test]
async fn list_instance_roles_follows_next_page_token() {
    let server = MockServer::start().await;
    let subject = new_subject();
    let subject_id = subject_id_string(&subject);

    Mock::given(method("GET"))
        .and(path("/relation-tuples"))
        .and(query_param_is_missing("page_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "relation_tuples": [
                {"namespace": "Instance", "object": "singleton", "relation": "admins", "subject_id": subject_id}
            ],
            "next_page_token": "page-2"
        })))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/relation-tuples"))
        .and(query_param("page_token", "page-2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "relation_tuples": [
                {"namespace": "Instance", "object": "singleton", "relation": "moderators", "subject_id": subject_id}
            ],
            "next_page_token": ""
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = keto_client(&server.uri());
    let roles = client.list_instance_roles(&subject).await.unwrap();

    assert_eq!(roles, vec![InstanceRole::Admin, InstanceRole::Moderator]);
    server.verify().await;
}

/// Given: Keto が 500 を返す
/// When: list_instance_roles を呼ぶ
/// Then: Ok(vec![]) にフォールバックせず Err(KernelError::Internal) を返す
#[tokio::test]
async fn list_instance_roles_returns_err_when_keto_returns_500() {
    let server = MockServer::start().await;
    let subject = new_subject();

    Mock::given(method("GET"))
        .and(path("/relation-tuples"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let client = keto_client(&server.uri());
    let result = client.list_instance_roles(&subject).await;

    let err = result.expect_err("500 response must not fall back to Ok(vec![])");
    assert_eq!(err.current_context(), &KernelError::Internal);
}

/// Given: Keto への接続が拒否される
/// When: list_instance_roles を呼ぶ
/// Then: Ok(vec![]) にフォールバックせず Err を返す
#[tokio::test]
async fn list_instance_roles_returns_err_when_connection_fails() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let client = keto_client(&format!("http://127.0.0.1:{port}"));
    let subject = new_subject();
    let result = client.list_instance_roles(&subject).await;

    assert!(
        result.is_err(),
        "connection failure must not fall back to Ok(vec![])"
    );
}

/// Given: Keto が不正な JSON を返す
/// When: list_instance_roles を呼ぶ
/// Then: Ok(vec![]) にフォールバックせず Err を返す
#[tokio::test]
async fn list_instance_roles_returns_err_when_response_is_invalid_json() {
    let server = MockServer::start().await;
    let subject = new_subject();

    Mock::given(method("GET"))
        .and(path("/relation-tuples"))
        .respond_with(ResponseTemplate::new(200).set_body_string("this is not json"))
        .mount(&server)
        .await;

    let client = keto_client(&server.uri());
    let result = client.list_instance_roles(&subject).await;

    assert!(
        result.is_err(),
        "invalid JSON must not fall back to Ok(vec![])"
    );
}

/// Given: Keto が空の relation_tuples を返す
/// When: list_instance_roles を呼ぶ
/// Then: エラーではなく Ok(vec![]) が返る
#[tokio::test]
async fn list_instance_roles_returns_empty_vec_when_no_tuples() {
    let server = MockServer::start().await;
    let subject = new_subject();

    Mock::given(method("GET"))
        .and(path("/relation-tuples"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "relation_tuples": [],
            "next_page_token": ""
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = keto_client(&server.uri());
    let roles = client.list_instance_roles(&subject).await.unwrap();

    assert_eq!(roles, Vec::<InstanceRole>::new());
    server.verify().await;
}
