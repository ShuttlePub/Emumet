use super::*;
use std::{
    future::Future,
    net::SocketAddr,
    pin::Pin,
    sync::atomic::{AtomicUsize, Ordering},
};
use wiremock::{Mock, MockServer, ResponseTemplate};
struct Resolver {
    addresses: Vec<SocketAddr>,
    calls: Arc<AtomicUsize>,
    delay: Duration,
}
impl HostResolver for Resolver {
    fn resolve<'a>(
        &'a self,
        _: &'a str,
        _: u16,
    ) -> Pin<Box<dyn Future<Output = std::io::Result<Vec<SocketAddr>>> + Send + 'a>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let addresses = self.addresses.clone();
        Box::pin(async move {
            tokio::time::sleep(self.delay).await;
            Ok(addresses)
        })
    }
}
fn transport() -> HttpPostRelayTransport {
    HttpPostRelayTransport::new(
        HomeOriginAllowlist::from_json("[\"https://home.example\"]").unwrap(),
    )
}
async fn send(
    t: &HttpPostRelayTransport,
    origin: &str,
    deadline: std::time::Instant,
) -> RelayHttpResult {
    t.send(&RelayHttpRequest {
        origin,
        envelope: b"{ \"a\": 1 }",
        headers: &[("content-type".into(), "application/json".into())],
        deadline,
    })
    .await
}
#[tokio::test]
async fn relay_http_status_retry_after_redirect_and_immutable_bytes() {
    for status in [204, 401, 403, 409, 302, 429, 503] {
        let peer = MockServer::start().await;
        Mock::given(wiremock::matchers::method("POST"))
            .respond_with(
                ResponseTemplate::new(status)
                    .insert_header("location", "http://127.0.0.1:1/leak")
                    .insert_header("retry-after", "45"),
            )
            .expect(1)
            .mount(&peer)
            .await;
        let t = transport().allowing_insecure_loopback();
        let result = send(
            &t,
            &peer.uri(),
            std::time::Instant::now() + Duration::from_secs(1),
        )
        .await;
        assert_eq!(
            result,
            RelayHttpResult::Response {
                status,
                retry_after_secs: if matches!(status, 429 | 503) {
                    Some(45)
                } else {
                    None
                }
            }
        );
        let requests = peer.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].body, b"{ \"a\": 1 }");
        assert_eq!(requests[0].url.path(), "/internal/v1/emumet/activities");
    }
}
#[tokio::test]
async fn relay_absolute_deadline_includes_dns_and_response_and_expired_does_not_connect() {
    let peer = MockServer::start().await;
    Mock::given(wiremock::matchers::method("POST"))
        .respond_with(ResponseTemplate::new(204).set_delay(Duration::from_secs(2)))
        .mount(&peer)
        .await;
    let t = transport().allowing_insecure_loopback();
    let start = std::time::Instant::now();
    assert_eq!(
        send(&t, &peer.uri(), start + Duration::from_millis(40)).await,
        RelayHttpResult::Transient
    );
    assert!(start.elapsed() < Duration::from_millis(500));
    let calls = Arc::new(AtomicUsize::new(0));
    let t = transport().with_resolver(Arc::new(Resolver {
        addresses: vec!["93.184.216.34:443".parse().unwrap()],
        calls: calls.clone(),
        delay: Duration::from_secs(2),
    }));
    let start = std::time::Instant::now();
    assert_eq!(
        send(
            &t,
            "https://home.example",
            start + Duration::from_millis(30)
        )
        .await,
        RelayHttpResult::Transient
    );
    assert!(start.elapsed() < Duration::from_millis(500));
    let before = calls.load(Ordering::SeqCst);
    assert_eq!(
        send(
            &t,
            "https://home.example",
            std::time::Instant::now() - Duration::from_secs(1)
        )
        .await,
        RelayHttpResult::Transient
    );
    assert_eq!(calls.load(Ordering::SeqCst), before);
}
#[tokio::test]
async fn relay_ssrf_internal_mixed_dns_and_trailing_dot_refused_even_when_allowlisted() {
    for origin in [
        "http://home.example",
        "https://127.0.0.1",
        "https://[::1]",
        "https://169.254.169.254",
        "https://10.0.0.1",
        "https://user:pw@home.example",
        "https://home.example.",
    ] {
        assert_eq!(
            send(
                &transport(),
                origin,
                std::time::Instant::now() + Duration::from_secs(1)
            )
            .await,
            RelayHttpResult::InvalidTarget,
            "{origin}"
        );
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let t = transport().with_resolver(Arc::new(Resolver {
        addresses: vec![
            "93.184.216.34:443".parse().unwrap(),
            "127.0.0.1:443".parse().unwrap(),
        ],
        calls: calls.clone(),
        delay: Duration::ZERO,
    }));
    assert!(t.origin_allowed("https://home.example"));
    assert_eq!(
        send(
            &t,
            "https://home.example",
            std::time::Instant::now() + Duration::from_secs(1)
        )
        .await,
        RelayHttpResult::InvalidTarget
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn relay_dns_is_pinned_and_never_resolved_again_by_http_client() {
    let peer = MockServer::start().await;
    Mock::given(wiremock::matchers::method("POST"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&peer)
        .await;
    let calls = Arc::new(AtomicUsize::new(0));
    let t = transport()
        .allowing_insecure_loopback()
        .with_resolver(Arc::new(Resolver {
            addresses: vec![*peer.address()],
            calls: calls.clone(),
            delay: Duration::ZERO,
        }));
    // This hostname cannot resolve through system DNS; only the validated override can connect.
    let origin = format!("http://no-system-dns.invalid:{}", peer.address().port());
    assert!(matches!(
        send(
            &t,
            &origin,
            std::time::Instant::now() + Duration::from_secs(2)
        )
        .await,
        RelayHttpResult::Response { status: 204, .. }
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn relay_cached_public_target_never_bypasses_a_rebound_private_dns_answer() {
    let calls = Arc::new(AtomicUsize::new(0));
    let t = transport().with_resolver(Arc::new(Resolver {
        addresses: vec!["127.0.0.1:443".parse().unwrap()],
        calls: calls.clone(),
        delay: Duration::ZERO,
    }));
    t.clients.lock().await.push(PinnedClient {
        origin: "https://home.example".into(),
        addresses: vec!["93.184.216.34:443".parse().unwrap()],
        client: reqwest::Client::new(),
        touched: std::time::Instant::now(),
    });
    assert_eq!(
        send(
            &t,
            "https://home.example",
            std::time::Instant::now() + Duration::from_secs(1)
        )
        .await,
        RelayHttpResult::InvalidTarget
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
