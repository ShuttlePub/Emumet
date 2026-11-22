//! B1: acquisition deadlines and fail-closed authentication across Ory outages.
use super::*;
use wiremock::{matchers::path, Mock, MockServer, ResponseTemplate};

fn token(keys: &TestKeys, issuer: &str, audience: &str, offset: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    encode_test_jwt(
        &AuthClaims {
            iss: issuer.into(),
            sub: "subject".into(),
            aud: OneOrMany::One(audience.into()),
            exp: (now + offset) as u64,
        },
        &keys.encoding_key,
        &keys.kid,
    )
}
async fn validate(cache: Arc<JwksCache>, issuer: &str, token: &str) -> Result<(), StatusCode> {
    let config = Arc::new(OidcConfig {
        issuer_url: issuer.into(),
        expected_audience: "emumet".into(),
        jwks_refetch_interval_secs: 0,
    });
    let mut request = Request::builder()
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    auth_middleware_core(config, cache, &mut request).await
}
fn cache(issuer: String, interval: Duration) -> Arc<JwksCache> {
    Arc::new(JwksCache::with_timeout(
        issuer,
        interval,
        Duration::from_millis(150),
    ))
}
async fn discovery(peer: &MockServer, uri: &str) {
    Mock::given(path("/.well-known/openid-configuration"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"jwks_uri": uri})),
        )
        .mount(peer)
        .await;
}

#[tokio::test]
async fn refused_discovery_and_uncached_authentication_fail_closed() {
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let issuer = format!("http://{}", socket.local_addr().unwrap());
    drop(socket);
    let cache = cache(issuer.clone(), Duration::from_secs(300));
    let keys = generate_test_keys();
    let token = token(&keys, &issuer, "emumet", 3600);
    tokio::time::timeout(Duration::from_secs(1), cache.try_init())
        .await
        .unwrap();
    assert_eq!(
        validate(cache, &issuer, &token).await,
        Err(StatusCode::UNAUTHORIZED)
    );
}

#[tokio::test]
async fn stalled_discovery_is_bounded_and_failed_attempts_are_single_flight() {
    let peer = MockServer::start().await;
    Mock::given(path("/.well-known/openid-configuration"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(2)))
        .expect(1)
        .mount(&peer)
        .await;
    let cache = cache(peer.uri(), Duration::from_secs(300));
    let mut requests = tokio::task::JoinSet::new();
    for _ in 0..16 {
        let cache = cache.clone();
        requests.spawn(async move { cache.get_key("missing").await.is_none() });
    }
    tokio::time::timeout(Duration::from_secs(1), async {
        while let Some(result) = requests.join_next().await {
            assert!(result.unwrap());
        }
    })
    .await
    .expect("mutex wait and stalled discovery must both be bounded");
    assert!(cache.inner.read().await.jwks.is_none());
}

#[tokio::test]
async fn stalled_jwks_headers_and_bodies_are_bounded() {
    use tokio::io::AsyncWriteExt;
    for send_headers in [false, true] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let uri = format!("http://{}", listener.local_addr().unwrap());
        let stalled = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            if send_headers {
                socket
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 200\r\n\r\n")
                    .await
                    .unwrap();
            }
            std::future::pending::<()>().await;
        });
        let peer = MockServer::start().await;
        discovery(&peer, &uri).await;
        let cache = cache(peer.uri(), Duration::from_secs(300));
        tokio::time::timeout(Duration::from_secs(1), cache.try_init())
            .await
            .unwrap();
        assert!(cache.inner.read().await.jwks.is_none());
        stalled.abort();
    }
}

#[tokio::test]
async fn uncached_acquisition_then_cached_outage_preserves_validation() {
    let peer = MockServer::start().await;
    let issuer = peer.uri();
    let keys = generate_test_keys();
    discovery(&peer, &format!("{issuer}/keys")).await;
    Mock::given(path("/keys"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&keys.jwk_set))
        .mount(&peer)
        .await;
    let cache = cache(issuer.clone(), Duration::ZERO);
    let valid = token(&keys, &issuer, "emumet", 3600);
    assert!(
        validate(cache.clone(), &issuer, &valid).await.is_ok(),
        "uncached discovery + fetch"
    );
    peer.reset().await;
    Mock::given(path("/keys"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(2)))
        .mount(&peer)
        .await;
    assert!(cache.get_key("unknown").await.is_none());
    assert!(
        validate(cache.clone(), &issuer, &valid).await.is_ok(),
        "failed refresh must not discard cached keys"
    );
    for (iss, aud, offset) in [
        ("wrong", "emumet", 3600),
        (issuer.as_str(), "wrong", 3600),
        (issuer.as_str(), "emumet", -3600),
    ] {
        assert_eq!(
            validate(cache.clone(), &issuer, &token(&keys, iss, aud, offset)).await,
            Err(StatusCode::UNAUTHORIZED)
        );
    }
    let unknown = TestKeys {
        kid: "unknown".into(),
        ..keys
    };
    assert_eq!(
        validate(cache, &issuer, &token(&unknown, &issuer, "emumet", 3600)).await,
        Err(StatusCode::UNAUTHORIZED)
    );
}

#[tokio::test]
async fn failed_or_empty_warmup_recovers_before_cached_refetch_interval() {
    let keys = generate_test_keys();
    for empty in [false, true] {
        let peer = MockServer::start().await;
        let issuer = peer.uri();
        discovery(&peer, &format!("{issuer}/keys")).await;
        let response = if empty {
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"keys": []}))
        } else {
            ResponseTemplate::new(503)
        };
        Mock::given(path("/keys"))
            .respond_with(response)
            .mount(&peer)
            .await;
        let cache = cache(issuer.clone(), Duration::from_secs(300));
        cache.try_init().await;
        let valid = token(&keys, &issuer, "emumet", 3600);
        assert_eq!(
            validate(cache.clone(), &issuer, &valid).await,
            Err(StatusCode::UNAUTHORIZED)
        );
        peer.reset().await;
        discovery(&peer, &format!("{issuer}/keys")).await;
        Mock::given(path("/keys"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&keys.jwk_set))
            .mount(&peer)
            .await;
        tokio::time::sleep(Duration::from_millis(1100)).await;
        assert!(
            validate(cache.clone(), &issuer, &valid).await.is_ok(),
            "cold cache recovers without the 300s populated-cache throttle"
        );
        let before = peer.received_requests().await.unwrap().len();
        assert!(cache.get_key("unknown").await.is_none());
        assert_eq!(
            peer.received_requests().await.unwrap().len(),
            before,
            "populated cache still throttles unknown kids"
        );
    }
}
