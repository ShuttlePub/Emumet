use super::{HttpProfileDeliverySender, HttpRemoteActorResolver, ProfileDeliveryNetworkSettings};
use crate::webhook::HostResolver;
use kernel::interfaces::repository::{
    AcctHandle, ProfileDeliveryRequest, ProfileDeliverySender, RemoteActorResolver,
};
use kernel::prelude::entity::{GraphActorUrl, ProfileSendObservation};
use kernel::KernelError;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct FixedResolver(Vec<SocketAddr>);

impl HostResolver for FixedResolver {
    fn resolve<'a>(
        &'a self,
        _host: &'a str,
        _port: u16,
    ) -> Pin<Box<dyn Future<Output = std::io::Result<Vec<SocketAddr>>> + Send + 'a>> {
        let addresses = self.0.clone();
        Box::pin(async move { Ok(addresses) })
    }
}

async fn post(
    sender: &HttpProfileDeliverySender,
    url: &str,
    body: &[u8],
) -> ProfileSendObservation {
    let headers = vec![
        (
            "content-type".to_string(),
            "application/activity+json".to_string(),
        ),
        (
            "signature".to_string(),
            "keyId=\"k\",signature=\"s\"".to_string(),
        ),
    ];
    sender
        .send(&ProfileDeliveryRequest {
            url,
            headers: &headers,
            body,
        })
        .await
}

#[tokio::test]
async fn production_sender_rejects_insecure_and_internal_destinations() {
    let sender = HttpProfileDeliverySender::new();
    for url in [
        "http://example.com/inbox",
        "https://127.0.0.1/inbox",
        "https://[::1]/inbox",
        "https://10.0.0.5/inbox",
        "https://169.254.169.254/inbox",
        "https://localhost/inbox",
        "https://user:secret@example.com/inbox",
        "not a url",
    ] {
        assert_eq!(
            post(&sender, url, b"{}").await,
            ProfileSendObservation::InvalidTarget,
            "{url}"
        );
    }
}

#[tokio::test]
async fn a_name_resolving_to_an_internal_address_is_refused_after_resolution() {
    let internal = HttpProfileDeliverySender::new().with_resolver(Arc::new(FixedResolver(vec![
        "10.1.2.3:443".parse().unwrap(),
    ])));
    assert_eq!(
        post(&internal, "https://remote.example/inbox", b"{}").await,
        ProfileSendObservation::InvalidTarget
    );
    let none = HttpProfileDeliverySender::new().with_resolver(Arc::new(FixedResolver(vec![])));
    assert_eq!(
        post(&none, "https://remote.example/inbox", b"{}").await,
        ProfileSendObservation::DnsFailed
    );
}

#[tokio::test]
async fn the_body_and_headers_are_sent_unchanged_and_the_status_is_observed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/inbox"))
        .respond_with(ResponseTemplate::new(202))
        .mount(&server)
        .await;
    let sender = HttpProfileDeliverySender::allowing_insecure_loopback();
    let body = b"{ \"b\":1,   \"a\":2 }";

    let observation = post(&sender, &format!("{}/inbox", server.uri()), body).await;

    assert_eq!(
        observation,
        ProfileSendObservation::Response {
            status: 202,
            retry_after: None
        }
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].body.as_slice(),
        body,
        "the body bytes are not re-serialized"
    );
    assert_eq!(
        requests[0]
            .headers
            .get("signature")
            .unwrap()
            .to_str()
            .unwrap(),
        "keyId=\"k\",signature=\"s\""
    );
    assert_eq!(
        requests[0]
            .headers
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap(),
        "application/activity+json"
    );
}

#[tokio::test]
async fn redirects_are_not_followed_and_retry_after_is_parsed() {
    let server = MockServer::start().await;
    let other = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/moved"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", format!("{}/inbox", other.uri())),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/busy"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "120"))
        .mount(&server)
        .await;
    let sender = HttpProfileDeliverySender::allowing_insecure_loopback();

    assert_eq!(
        post(&sender, &format!("{}/moved", server.uri()), b"{}").await,
        ProfileSendObservation::Response {
            status: 302,
            retry_after: None
        }
    );
    assert!(other.received_requests().await.unwrap().is_empty());
    assert_eq!(
        post(&sender, &format!("{}/busy", server.uri()), b"{}").await,
        ProfileSendObservation::Response {
            status: 429,
            retry_after: Some(time::Duration::seconds(120))
        }
    );
}

#[tokio::test]
async fn a_slow_peer_times_out_and_a_closed_port_fails_to_connect() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(202).set_delay(Duration::from_secs(3)))
        .mount(&server)
        .await;
    let sender = HttpProfileDeliverySender::allowing_insecure_loopback().with_settings(
        ProfileDeliveryNetworkSettings {
            total_timeout: Duration::from_millis(200),
            connect_timeout: Duration::from_millis(200),
            ..ProfileDeliveryNetworkSettings::default()
        },
    );

    assert_eq!(
        post(&sender, &format!("{}/inbox", server.uri()), b"{}").await,
        ProfileSendObservation::Timeout
    );
    let closed = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    assert_eq!(
        post(&sender, &format!("http://127.0.0.1:{closed}/inbox"), b"{}").await,
        ProfileSendObservation::ConnectFailed
    );
}

async fn actor(server: &MockServer, actor_path: &str, document: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path(actor_path))
        .respond_with(ResponseTemplate::new(200).set_body_json(document))
        .mount(server)
        .await;
}

fn url(value: String) -> GraphActorUrl {
    GraphActorUrl::new(value).unwrap()
}

#[tokio::test]
async fn the_resolver_returns_the_inbox_of_the_actor_document() {
    let server = MockServer::start().await;
    let id = format!("{}/users/bob", server.uri());
    actor(
        &server,
        "/users/bob",
        serde_json::json!({"id": id, "inbox": format!("{}/users/bob/inbox", server.uri())}),
    )
    .await;
    let resolver = HttpRemoteActorResolver::allowing_insecure_loopback();

    let info = resolver.resolve(&url(id.clone())).await.unwrap();

    assert_eq!(info.actor_url.as_str(), id);
    assert_eq!(
        info.inbox.as_str(),
        format!("{}/users/bob/inbox", server.uri())
    );
}

#[tokio::test]
async fn the_resolver_rejects_mismatched_ids_missing_inboxes_and_unknown_actors() {
    let server = MockServer::start().await;
    actor(
        &server,
        "/users/impostor",
        serde_json::json!({"id": "https://elsewhere.example/users/real", "inbox": "https://elsewhere.example/inbox"}),
    )
    .await;
    actor(
        &server,
        "/users/no-inbox",
        serde_json::json!({"id": format!("{}/users/no-inbox", server.uri())}),
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/users/text"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>"))
        .mount(&server)
        .await;
    let resolver = HttpRemoteActorResolver::allowing_insecure_loopback();

    for actor_path in ["/users/impostor", "/users/no-inbox", "/users/text"] {
        let error = resolver
            .resolve(&url(format!("{}{actor_path}", server.uri())))
            .await
            .unwrap_err();
        assert_eq!(
            error.current_context(),
            &KernelError::Rejected,
            "{actor_path}"
        );
    }
    let error = resolver
        .resolve(&url(format!("{}/users/missing", server.uri())))
        .await
        .unwrap_err();
    assert_eq!(error.current_context(), &KernelError::NotFound);
}

#[tokio::test]
async fn the_production_resolver_refuses_internal_destinations() {
    let resolver = HttpRemoteActorResolver::new();
    for target in [
        "https://127.0.0.1/users/a",
        "http://example.com/users/a",
        "https://localhost/users/a",
    ] {
        let error = resolver
            .resolve(&url(target.to_string()))
            .await
            .unwrap_err();
        assert_eq!(error.current_context(), &KernelError::Rejected, "{target}");
    }
}

/// 名前解決の回数を数え、常に公開アドレスを返す resolver (検査が名前解決の前に拒否することの確認用)。
struct CountingResolver(Arc<AtomicUsize>);

impl HostResolver for CountingResolver {
    fn resolve<'a>(
        &'a self,
        _host: &'a str,
        _port: u16,
    ) -> Pin<Box<dyn Future<Output = std::io::Result<Vec<SocketAddr>>> + Send + 'a>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(vec!["93.184.216.34:443".parse().unwrap()]) })
    }
}

#[tokio::test]
async fn hosts_with_a_trailing_dot_are_refused_by_the_sender_and_the_resolver() {
    // 検査・DNS 固定の名前と送信 URL の host がずれると、固定が効かず通常 DNS へフォールバックして
    // 内部アドレスへ接続されうる。末尾ドット付きの host は名前解決より前に拒否する。
    let lookups = Arc::new(AtomicUsize::new(0));
    let sender =
        HttpProfileDeliverySender::new().with_resolver(Arc::new(CountingResolver(lookups.clone())));
    for url in [
        "https://rebind.example./inbox",
        "https://rebind.example.:443/inbox",
        "https://a.b.example./inbox",
    ] {
        assert_eq!(
            post(&sender, url, b"{}").await,
            ProfileSendObservation::InvalidTarget,
            "{url}"
        );
    }
    let resolver =
        HttpRemoteActorResolver::new().with_resolver(Arc::new(CountingResolver(lookups.clone())));
    let error = resolver
        .resolve(&url("https://rebind.example./users/a".to_string()))
        .await
        .unwrap_err();
    assert_eq!(error.current_context(), &KernelError::Rejected);
    assert_eq!(
        lookups.load(Ordering::SeqCst),
        0,
        "rejected before any name resolution"
    );

    // 検証用 (http・loopback 許可) の構築でも末尾ドットは許さない。
    let server = MockServer::start().await;
    let port = server.address().port();
    let loopback = HttpProfileDeliverySender::allowing_insecure_loopback();
    assert_eq!(
        post(&loopback, &format!("http://localhost.:{port}/inbox"), b"{}").await,
        ProfileSendObservation::InvalidTarget
    );
    assert!(server.received_requests().await.unwrap().is_empty());
}

async fn webfinger(server: &MockServer, resource: &str, document: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path("/.well-known/webfinger"))
        .and(query_param("resource", resource))
        .respond_with(ResponseTemplate::new(200).set_body_json(document))
        .mount(server)
        .await;
}

#[tokio::test]
async fn webfinger_resolves_an_acct_handle_to_the_activitypub_self_link() {
    let server = MockServer::start().await;
    let domain = server.address().to_string();
    let resolver = HttpRemoteActorResolver::allowing_insecure_loopback();
    webfinger(
        &server,
        &format!("acct:bob@{domain}"),
        serde_json::json!({
            "subject": format!("acct:bob@{domain}"),
            "links": [
                {"rel": "http://webfinger.net/rel/profile-page", "href": "https://x.example/@bob"},
                {"rel": "self", "type": "application/activity+json", "href": "https://x.example/users/bob"}
            ]
        }),
    )
    .await;
    webfinger(
        &server,
        &format!("acct:evil@{domain}"),
        serde_json::json!({
            "subject": "acct:someone-else@x.example",
            "links": [{"rel": "self", "type": "application/activity+json", "href": "https://x.example/users/evil"}]
        }),
    )
    .await;
    webfinger(
        &server,
        &format!("acct:nolink@{domain}"),
        serde_json::json!({"subject": format!("acct:nolink@{domain}"), "links": []}),
    )
    .await;

    let url = resolver
        .resolve_acct(&AcctHandle::parse(&format!("acct:bob@{domain}")).unwrap())
        .await
        .unwrap();
    assert_eq!(url.as_str(), "https://x.example/users/bob");

    for (handle, expected) in [
        (format!("evil@{domain}"), KernelError::Rejected),
        (format!("nolink@{domain}"), KernelError::Rejected),
        (format!("ghost@{domain}"), KernelError::NotFound),
    ] {
        let error = resolver
            .resolve_acct(&AcctHandle::parse(&handle).unwrap())
            .await
            .unwrap_err();
        assert_eq!(error.current_context(), &expected, "{handle}");
    }
}

#[tokio::test]
async fn the_production_resolver_refuses_internal_acct_domains() {
    let resolver = HttpRemoteActorResolver::new();
    for handle in [
        "bob@127.0.0.1",
        "bob@localhost",
        "bob@10.0.0.7:8443",
        "bob@[::1]",
    ] {
        let parsed = match AcctHandle::parse(handle) {
            Ok(parsed) => parsed,
            Err(_) => continue,
        };
        let error = resolver.resolve_acct(&parsed).await.unwrap_err();
        assert_eq!(error.current_context(), &KernelError::Rejected, "{handle}");
    }
}
