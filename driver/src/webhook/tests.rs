use super::endpoint::{is_forbidden_ip, load_secret, parse_endpoint, EndpointHost, EndpointMode};
use super::signature::{self, SignatureError};
use super::*;
use kernel::interfaces::repository::{WebhookRequest, WebhookSender};
use kernel::prelude::entity::{
    WebhookConfigIssue, WebhookConfigStatus, WebhookEventType, WebhookSendObservation,
};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SECRET: &str = "whsec_test_secret_0123456789abcdef";
const BODY: &str = r#"{"version":1,"event_id":"123456789","type":"outage_started"}"#;

// ---- 署名 ----------------------------------------------------------------------------------

/// 文書 (docs/registration-gate-webhook.md) のテストベクタ。perl の Digest::SHA で独立に算出した値。
#[test]
fn signature_matches_the_documented_test_vector() {
    let signature = signature::sign(SECRET.as_bytes(), 1_700_000_000, BODY.as_bytes());
    assert_eq!(
        signature,
        "v1=153d6760b728623755e694d34f231eef1aa23a335378c41fec53ac2d05a19771"
    );
}

#[test]
fn signature_verifies_and_rejects_tampering_replay_and_garbage() {
    let timestamp = 1_700_000_000;
    let header = signature::sign(SECRET.as_bytes(), timestamp, BODY.as_bytes());
    let verify = |secret: &str, header: &str, ts: i64, body: &str, now: i64| {
        signature::verify(secret.as_bytes(), header, ts, body.as_bytes(), now, 300)
    };

    assert_eq!(
        verify(SECRET, &header, timestamp, BODY, timestamp + 299),
        Ok(())
    );
    assert_eq!(
        verify(SECRET, &header, timestamp, BODY, timestamp - 299),
        Ok(())
    );
    assert_eq!(
        verify(SECRET, &header, timestamp, BODY, timestamp + 301),
        Err(SignatureError::TimestampOutOfTolerance)
    );
    assert_eq!(
        verify(SECRET, &header, timestamp, &format!("{BODY} "), timestamp),
        Err(SignatureError::Mismatch)
    );
    assert_eq!(
        verify(
            "another-secret-0123456789",
            &header,
            timestamp,
            BODY,
            timestamp
        ),
        Err(SignatureError::Mismatch)
    );
    // timestamp も署名対象 (別 timestamp での再利用は不一致)
    assert_eq!(
        verify(SECRET, &header, timestamp + 1, BODY, timestamp + 1),
        Err(SignatureError::Mismatch)
    );
    for garbage in ["", "v1=", "v1=zz", "v1=abc", "v2=00", "sha256=00", "153d"] {
        assert_eq!(
            verify(SECRET, garbage, timestamp, BODY, timestamp),
            Err(SignatureError::Malformed),
            "{garbage}"
        );
    }
}

// ---- endpoint 設定・SSRF fixture --------------------------------------------------------------

fn strict(url: &str) -> Result<super::endpoint::WebhookEndpoint, WebhookConfigIssue> {
    parse_endpoint(url, EndpointMode::Strict)
}

#[test]
fn endpoint_fixtures_are_rejected_in_production_mode() {
    let cases: &[(&str, WebhookConfigIssue)] = &[
        // https 以外
        (
            "http://hooks.example.com/",
            WebhookConfigIssue::UnsupportedScheme,
        ),
        (
            "ftp://hooks.example.com/",
            WebhookConfigIssue::UnsupportedScheme,
        ),
        ("file:///etc/passwd", WebhookConfigIssue::UnsupportedScheme),
        // 認証情報
        (
            "https://user:pass@hooks.example.com/",
            WebhookConfigIssue::CredentialsInUrl,
        ),
        (
            "https://user@hooks.example.com/",
            WebhookConfigIssue::CredentialsInUrl,
        ),
        // 解釈できない URL・zone
        ("not a url", WebhookConfigIssue::InvalidUrl),
        ("https://", WebhookConfigIssue::InvalidUrl),
        ("https://[fe80::1%25eth0]/", WebhookConfigIssue::InvalidUrl),
        ("https://[fe80::1%eth0]/", WebhookConfigIssue::InvalidUrl),
        // localhost 系 (末尾ドット・大文字・サブドメイン)
        (
            "https://localhost/",
            WebhookConfigIssue::ForbiddenDestination,
        ),
        (
            "https://LOCALHOST/",
            WebhookConfigIssue::ForbiddenDestination,
        ),
        (
            "https://localhost./",
            WebhookConfigIssue::ForbiddenDestination,
        ),
        (
            "https://localhost../",
            WebhookConfigIssue::ForbiddenDestination,
        ),
        (
            "https://foo.localhost/",
            WebhookConfigIssue::ForbiddenDestination,
        ),
        (
            "https://metadata.google.internal/",
            WebhookConfigIssue::ForbiddenDestination,
        ),
        // IPv4 リテラルと表記ゆれ
        (
            "https://127.0.0.1/",
            WebhookConfigIssue::ForbiddenDestination,
        ),
        (
            "https://127.0.0.1./",
            WebhookConfigIssue::ForbiddenDestination,
        ),
        (
            "https://2130706433/",
            WebhookConfigIssue::ForbiddenDestination,
        ),
        (
            "https://0x7f000001/",
            WebhookConfigIssue::ForbiddenDestination,
        ),
        (
            "https://0x7f.0.0.1/",
            WebhookConfigIssue::ForbiddenDestination,
        ),
        ("https://127.1/", WebhookConfigIssue::ForbiddenDestination),
        (
            "https://0177.0.0.1/",
            WebhookConfigIssue::ForbiddenDestination,
        ),
        ("https://0/", WebhookConfigIssue::ForbiddenDestination),
        (
            "https://10.0.0.1/",
            WebhookConfigIssue::ForbiddenDestination,
        ),
        (
            "https://172.16.0.1/",
            WebhookConfigIssue::ForbiddenDestination,
        ),
        (
            "https://192.168.1.1/",
            WebhookConfigIssue::ForbiddenDestination,
        ),
        (
            "https://100.64.0.1/",
            WebhookConfigIssue::ForbiddenDestination,
        ),
        (
            "https://198.18.0.1/",
            WebhookConfigIssue::ForbiddenDestination,
        ),
        (
            "https://169.254.169.254/latest/meta-data/",
            WebhookConfigIssue::ForbiddenDestination,
        ),
        (
            "https://224.0.0.1/",
            WebhookConfigIssue::ForbiddenDestination,
        ),
        (
            "https://255.255.255.255/",
            WebhookConfigIssue::ForbiddenDestination,
        ),
        (
            "https://192.88.99.1/",
            WebhookConfigIssue::ForbiddenDestination,
        ),
        // IPv6
        ("https://[::1]/", WebhookConfigIssue::ForbiddenDestination),
        ("https://[::]/", WebhookConfigIssue::ForbiddenDestination),
        (
            "https://[::ffff:127.0.0.1]/",
            WebhookConfigIssue::ForbiddenDestination,
        ),
        (
            "https://[::ffff:7f00:1]/",
            WebhookConfigIssue::ForbiddenDestination,
        ),
        (
            "https://[::ffff:8.8.8.8]/",
            WebhookConfigIssue::ForbiddenDestination,
        ),
        (
            "https://[::127.0.0.1]/",
            WebhookConfigIssue::ForbiddenDestination,
        ),
        (
            "https://[64:ff9b::7f00:1]/",
            WebhookConfigIssue::ForbiddenDestination,
        ),
        (
            "https://[fd00:ec2::254]/",
            WebhookConfigIssue::ForbiddenDestination,
        ),
        (
            "https://[fc00::1]/",
            WebhookConfigIssue::ForbiddenDestination,
        ),
        (
            "https://[fe80::1]/",
            WebhookConfigIssue::ForbiddenDestination,
        ),
        (
            "https://[fec0::1]/",
            WebhookConfigIssue::ForbiddenDestination,
        ),
        (
            "https://[ff02::1]/",
            WebhookConfigIssue::ForbiddenDestination,
        ),
        (
            "https://[2001:db8::1]/",
            WebhookConfigIssue::ForbiddenDestination,
        ),
        (
            "https://[2002:7f00:1::]/",
            WebhookConfigIssue::ForbiddenDestination,
        ),
    ];
    for (url, expected) in cases {
        assert_eq!(
            strict(url).map(|endpoint| endpoint.display_host()),
            Err(*expected),
            "{url}"
        );
    }
}

#[test]
fn public_endpoints_are_accepted_and_normalized() {
    let endpoint = strict("https://Hooks.Example.com./path/to?token=abc#frag").unwrap();
    assert_eq!(
        endpoint.host,
        EndpointHost::Domain("hooks.example.com".into())
    );
    assert_eq!(endpoint.port, 443);
    assert_eq!(endpoint.url.host_str(), Some("hooks.example.com"));
    assert_eq!(endpoint.display_host(), "hooks.example.com");

    let endpoint = strict("https://hooks.example.com:8443/x").unwrap();
    assert_eq!(endpoint.port, 8443);

    assert!(strict("https://8.8.8.8/").is_ok());
    assert!(strict("https://[2606:4700:4700::1111]/hook").is_ok());
}

#[test]
fn forbidden_ip_table() {
    for blocked in [
        "127.0.0.1",
        "10.1.2.3",
        "172.31.255.255",
        "192.168.0.1",
        "169.254.169.254",
        "100.127.255.255",
        "0.0.0.0",
        "239.255.255.250",
        "240.0.0.1",
        "::1",
        "::",
        "::ffff:10.0.0.1",
        "fe80::1",
        "fd12:3456::1",
        "ff00::1",
        "64:ff9b::a00:1",
        "2001:db8::1",
        "2001::1",
        "2002::1",
    ] {
        assert!(is_forbidden_ip(blocked.parse().unwrap()), "{blocked}");
    }
    for allowed in [
        "8.8.8.8",
        "1.1.1.1",
        "93.184.216.34",
        "2606:4700:4700::1111",
    ] {
        assert!(!is_forbidden_ip(allowed.parse().unwrap()), "{allowed}");
    }
}

#[test]
fn the_ap_fetch_ssrf_check_is_unchanged_for_mapped_and_private_addresses() {
    use crate::http_signing::ssrf::{is_blocked_ipv4, is_blocked_ipv6};
    assert!(is_blocked_ipv4("10.0.0.1".parse().unwrap()));
    assert!(is_blocked_ipv6("::ffff:10.0.0.1".parse().unwrap()));
    assert!(!is_blocked_ipv4("8.8.8.8".parse().unwrap()));
}

// ---- secret ------------------------------------------------------------------------------

#[test]
fn secret_loading_rules() {
    assert!(load_secret(None, None).unwrap().is_none());
    assert!(load_secret(Some("  "), Some("")).unwrap().is_none());
    assert_eq!(
        load_secret(Some("short"), None).unwrap_err(),
        WebhookConfigIssue::SecretTooShort
    );
    assert_eq!(
        load_secret(Some(SECRET), Some("/some/file")).unwrap_err(),
        WebhookConfigIssue::AmbiguousSecretSource
    );
    assert_eq!(
        load_secret(None, Some("/definitely/not/here")).unwrap_err(),
        WebhookConfigIssue::SecretUnreadable
    );
    let secret = load_secret(Some(&format!("  {SECRET}\n")), None)
        .unwrap()
        .unwrap();
    assert_eq!(secret.as_bytes(), SECRET.as_bytes());

    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("secret");
    std::fs::write(&file, format!("{SECRET}\n")).unwrap();
    let secret = load_secret(None, file.to_str()).unwrap().unwrap();
    assert_eq!(secret.as_bytes(), SECRET.as_bytes());
    std::fs::write(&file, "\n").unwrap();
    assert_eq!(
        load_secret(None, file.to_str()).unwrap_err(),
        WebhookConfigIssue::SecretUnreadable
    );

    let debug = format!("{secret:?}");
    assert!(!debug.contains(SECRET));
    assert!(debug.contains("redacted"));
}

#[test]
fn sender_status_reflects_configuration_without_leaking_secret_or_path() {
    assert_eq!(
        HttpWebhookSender::from_values(None, None, None).status(),
        WebhookConfigStatus::Disabled
    );
    assert_eq!(
        HttpWebhookSender::from_values(Some("https://hooks.example.com/"), None, None).status(),
        WebhookConfigStatus::Misconfigured {
            issue: WebhookConfigIssue::MissingSecret
        }
    );
    assert_eq!(
        HttpWebhookSender::from_values(None, Some(SECRET), None).status(),
        WebhookConfigStatus::Misconfigured {
            issue: WebhookConfigIssue::MissingUrl
        }
    );
    assert_eq!(
        HttpWebhookSender::from_values(Some("http://hooks.example.com/"), Some(SECRET), None)
            .status(),
        WebhookConfigStatus::Misconfigured {
            issue: WebhookConfigIssue::UnsupportedScheme
        }
    );
    assert_eq!(
        HttpWebhookSender::from_values(Some("https://10.0.0.1/"), Some(SECRET), None).status(),
        WebhookConfigStatus::Misconfigured {
            issue: WebhookConfigIssue::ForbiddenDestination
        }
    );
    let enabled = HttpWebhookSender::from_values(
        Some("https://hooks.example.com:8443/secret/path?token=urlsecret"),
        Some(SECRET),
        None,
    );
    let status = enabled.status();
    assert_eq!(
        status,
        WebhookConfigStatus::Enabled {
            host: "hooks.example.com".into()
        }
    );
    for rendered in [
        serde_json::to_string(&status).unwrap(),
        format!("{status:?}"),
        format!("{enabled:?}"),
    ] {
        for leaked in [SECRET, "urlsecret", "secret/path", "8443", "token"] {
            assert!(!rendered.contains(leaked), "{rendered} leaks {leaked}");
        }
    }
}

#[tokio::test]
async fn a_misconfigured_or_disabled_sender_never_sends() {
    for sender in [
        HttpWebhookSender::from_values(None, None, None),
        HttpWebhookSender::from_values(Some("https://10.0.0.1/"), Some(SECRET), None),
    ] {
        assert_eq!(
            sender.send(&request("1", 1)).await,
            WebhookSendObservation::ConfigInvalid
        );
    }
}

// ---- 送信 (fake receiver) ---------------------------------------------------------------------

fn request(event_id: &str, attempt: u32) -> WebhookRequest<'_> {
    WebhookRequest {
        event_id,
        event_type: WebhookEventType::OutageStarted,
        attempt,
        body: BODY.as_bytes(),
    }
}

fn fast_settings() -> WebhookNetworkSettings {
    WebhookNetworkSettings {
        resolve_timeout: Duration::from_millis(500),
        connect_timeout: Duration::from_millis(500),
        total_timeout: Duration::from_millis(800),
        max_response_bytes: 64 * 1024,
    }
}

fn test_sender(server: &MockServer) -> HttpWebhookSender {
    HttpWebhookSender::from_values_allowing_insecure_loopback(
        &format!("{}/hook", server.uri()),
        SECRET,
    )
    .with_settings(fast_settings())
}

#[tokio::test]
async fn delivers_signed_request_with_stable_headers() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/hook"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    let sender = test_sender(&server);

    let observation = sender.send(&request("123456789", 3)).await;
    assert_eq!(
        observation,
        WebhookSendObservation::Response {
            status: 204,
            retry_after: None
        }
    );

    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 1);
    let received = &received[0];
    let header = |name: &str| {
        received
            .headers
            .get(name)
            .unwrap_or_else(|| panic!("missing header {name}"))
            .to_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(header(HEADER_EVENT_ID), "123456789");
    assert_eq!(header(HEADER_EVENT_TYPE), "outage_started");
    assert_eq!(header(HEADER_DELIVERY_ATTEMPT), "3");
    assert_eq!(header("content-type"), "application/json");
    assert_eq!(received.body, BODY.as_bytes());
    let timestamp: i64 = header(HEADER_TIMESTAMP).parse().unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    assert!((now - timestamp).abs() < 10);
    assert_eq!(
        signature::verify(
            SECRET.as_bytes(),
            &header(HEADER_SIGNATURE),
            timestamp,
            &received.body,
            now,
            300
        ),
        Ok(())
    );
    assert!(!header(HEADER_SIGNATURE).contains(SECRET));
}

#[tokio::test]
async fn status_codes_are_reported_as_responses() {
    for status in [200u16, 301, 400, 404, 408, 500, 503] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(status))
            .mount(&server)
            .await;
        assert_eq!(
            test_sender(&server).send(&request("1", 1)).await,
            WebhookSendObservation::Response {
                status,
                retry_after: None
            },
            "status {status}"
        );
    }
}

#[tokio::test]
async fn retry_after_seconds_and_http_date_are_parsed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "120"))
        .mount(&server)
        .await;
    assert_eq!(
        test_sender(&server).send(&request("1", 1)).await,
        WebhookSendObservation::Response {
            status: 429,
            retry_after: Some(time::Duration::seconds(120))
        }
    );

    let server = MockServer::start().await;
    let at = std::time::SystemTime::now() + Duration::from_secs(600);
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(503).insert_header("Retry-After", httpdate::fmt_http_date(at)),
        )
        .mount(&server)
        .await;
    match test_sender(&server).send(&request("1", 1)).await {
        WebhookSendObservation::Response {
            status: 503,
            retry_after: Some(delay),
        } => assert!(
            delay > time::Duration::seconds(590) && delay <= time::Duration::seconds(600),
            "{delay}"
        ),
        other => panic!("{other:?}"),
    }

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "soon"))
        .mount(&server)
        .await;
    assert_eq!(
        test_sender(&server).send(&request("1", 1)).await,
        WebhookSendObservation::Response {
            status: 429,
            retry_after: None
        }
    );
}

#[tokio::test]
async fn redirects_are_never_followed() {
    let internal = MockServer::start().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(200))
        .mount(&internal)
        .await;
    for status in [301u16, 302, 303, 307, 308] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(status).insert_header("Location", internal.uri().as_str()),
            )
            .mount(&server)
            .await;
        assert_eq!(
            test_sender(&server).send(&request("1", 1)).await,
            WebhookSendObservation::Response {
                status,
                retry_after: None
            }
        );
    }
    assert!(internal.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn slow_receiver_times_out() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(5)))
        .mount(&server)
        .await;
    assert_eq!(
        test_sender(&server).send(&request("1", 1)).await,
        WebhookSendObservation::Timeout
    );
}

#[tokio::test]
async fn refused_connection_is_a_connect_failure() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let sender = HttpWebhookSender::from_values_allowing_insecure_loopback(
        &format!("http://127.0.0.1:{port}/hook"),
        SECRET,
    )
    .with_settings(fast_settings());
    assert_eq!(
        sender.send(&request("1", 1)).await,
        WebhookSendObservation::ConnectFailed
    );
}

#[tokio::test]
async fn a_huge_response_body_is_discarded_within_the_limit() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'x'; 8 * 1024 * 1024]))
        .mount(&server)
        .await;
    let started = std::time::Instant::now();
    assert_eq!(
        test_sender(&server).send(&request("1", 1)).await,
        WebhookSendObservation::Response {
            status: 200,
            retry_after: None
        }
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}

// ---- 名前解決・DNS rebinding ---------------------------------------------------------------

struct ScriptedResolver {
    answers: std::sync::Mutex<Vec<std::io::Result<Vec<SocketAddr>>>>,
    calls: AtomicUsize,
}

impl ScriptedResolver {
    fn new(answers: Vec<std::io::Result<Vec<SocketAddr>>>) -> Arc<Self> {
        Arc::new(Self {
            answers: std::sync::Mutex::new(answers),
            calls: AtomicUsize::new(0),
        })
    }
}

impl HostResolver for ScriptedResolver {
    fn resolve<'a>(
        &'a self,
        _host: &'a str,
        _port: u16,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = std::io::Result<Vec<SocketAddr>>> + Send + 'a>,
    > {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let mut answers = self.answers.lock().unwrap();
            if answers.len() > 1 {
                answers.remove(0)
            } else {
                match &answers[0] {
                    Ok(addresses) => Ok(addresses.clone()),
                    Err(error) => Err(std::io::Error::new(error.kind(), "scripted")),
                }
            }
        })
    }
}

fn addr(ip: &str) -> SocketAddr {
    SocketAddr::new(ip.parse::<IpAddr>().unwrap(), 443)
}

fn strict_sender_with(resolver: Arc<ScriptedResolver>) -> HttpWebhookSender {
    HttpWebhookSender::from_values(Some("https://hooks.example.com/hook"), Some(SECRET), None)
        .with_settings(fast_settings())
        .with_resolver(resolver)
}

#[tokio::test]
async fn hostnames_resolving_to_internal_addresses_are_blocked_as_ssrf() {
    for internal in [
        "127.0.0.1",
        "10.0.0.5",
        "192.168.1.10",
        "169.254.169.254",
        "100.64.0.1",
        "::1",
        "fd00:ec2::254",
        "::ffff:127.0.0.1",
        "fe80::1",
    ] {
        let resolver = ScriptedResolver::new(vec![Ok(vec![addr(internal)])]);
        let sender = strict_sender_with(resolver.clone());
        assert_eq!(
            sender.send(&request("1", 1)).await,
            WebhookSendObservation::SsrfBlocked,
            "{internal}"
        );
        assert_eq!(resolver.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn one_internal_address_among_public_ones_blocks_the_destination() {
    let resolver = ScriptedResolver::new(vec![Ok(vec![
        addr("8.8.8.8"),
        addr("2606:4700:4700::1111"),
        addr("10.0.0.5"),
    ])]);
    assert_eq!(
        strict_sender_with(resolver).send(&request("1", 1)).await,
        WebhookSendObservation::SsrfBlocked
    );
}

#[tokio::test]
async fn every_attempt_resolves_again_and_checks_the_new_answer() {
    let resolver = ScriptedResolver::new(vec![
        Ok(vec![addr("127.0.0.1")]),
        Ok(vec![addr("169.254.169.254")]),
        Ok(vec![addr("::1")]),
    ]);
    let sender = strict_sender_with(resolver.clone());
    for attempt in 1..=3 {
        assert_eq!(
            sender.send(&request("1", attempt)).await,
            WebhookSendObservation::SsrfBlocked
        );
    }
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn dns_failures_are_transient() {
    for answer in [
        Err(std::io::Error::other("nxdomain")),
        Ok(Vec::<SocketAddr>::new()),
    ] {
        let resolver = ScriptedResolver::new(vec![answer]);
        assert_eq!(
            strict_sender_with(resolver).send(&request("1", 1)).await,
            WebhookSendObservation::DnsFailed
        );
    }
}

#[tokio::test]
async fn the_connection_is_pinned_to_the_checked_address() {
    // 実在しない名前 (.invalid) を、検査した 127.0.0.1:<port> へ固定して接続できる = 接続時に再解決していない。
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let port = server.address().port();
    let resolver = ScriptedResolver::new(vec![Ok(vec![SocketAddr::new(
        "127.0.0.1".parse().unwrap(),
        port,
    )])]);
    let sender = HttpWebhookSender::from_values_allowing_insecure_loopback(
        &format!("http://rebind-pinned.invalid:{port}/hook"),
        SECRET,
    )
    .with_settings(fast_settings())
    .with_resolver(resolver.clone());
    assert_eq!(
        sender.send(&request("1", 1)).await,
        WebhookSendObservation::Response {
            status: 200,
            retry_after: None
        }
    );
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 1);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

// ---- TLS: 接続固定 + 元のホスト名での証明書検証 ----------------------------------------------

struct TlsReceiver {
    port: u16,
    requests: Arc<AtomicUsize>,
    certificate_pem: String,
}

fn self_signed_certificate(name: &str) -> (String, String) {
    use openssl::asn1::Asn1Time;
    use openssl::bn::BigNum;
    use openssl::hash::MessageDigest;
    use openssl::pkey::PKey;
    use openssl::rsa::Rsa;
    use openssl::x509::extension::SubjectAlternativeName;
    use openssl::x509::{X509Builder, X509NameBuilder};

    let key = PKey::from_rsa(Rsa::generate(2048).unwrap()).unwrap();
    let mut subject = X509NameBuilder::new().unwrap();
    subject.append_entry_by_text("CN", name).unwrap();
    let subject = subject.build();
    let mut builder = X509Builder::new().unwrap();
    builder.set_version(2).unwrap();
    let serial = BigNum::from_u32(1).unwrap().to_asn1_integer().unwrap();
    builder.set_serial_number(&serial).unwrap();
    builder.set_subject_name(&subject).unwrap();
    builder.set_issuer_name(&subject).unwrap();
    builder.set_pubkey(&key).unwrap();
    builder
        .set_not_before(&Asn1Time::days_from_now(0).unwrap())
        .unwrap();
    builder
        .set_not_after(&Asn1Time::days_from_now(2).unwrap())
        .unwrap();
    let san = SubjectAlternativeName::new()
        .dns(name)
        .build(&builder.x509v3_context(None, None))
        .unwrap();
    builder.append_extension(san).unwrap();
    builder.sign(&key, MessageDigest::sha256()).unwrap();
    let certificate = builder.build();
    (
        String::from_utf8(certificate.to_pem().unwrap()).unwrap(),
        String::from_utf8(key.private_key_to_pem_pkcs8().unwrap()).unwrap(),
    )
}

async fn start_tls_receiver(certificate_name: &str) -> TlsReceiver {
    let (certificate_pem, key_pem) = self_signed_certificate(certificate_name);
    let identity =
        native_tls::Identity::from_pkcs8(certificate_pem.as_bytes(), key_pem.as_bytes()).unwrap();
    let acceptor =
        tokio_native_tls::TlsAcceptor::from(native_tls::TlsAcceptor::new(identity).unwrap());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests = Arc::new(AtomicUsize::new(0));
    let counter = requests.clone();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        loop {
            let Ok((tcp, _)) = listener.accept().await else {
                break;
            };
            let acceptor = acceptor.clone();
            let counter = counter.clone();
            tokio::spawn(async move {
                let Ok(mut stream) = acceptor.accept(tcp).await else {
                    return;
                };
                let mut received = Vec::new();
                let mut buffer = [0u8; 4096];
                while !received.windows(4).any(|window| window == b"\r\n\r\n") {
                    match stream.read(&mut buffer).await {
                        Ok(0) | Err(_) => return,
                        Ok(read) => received.extend_from_slice(&buffer[..read]),
                    }
                }
                counter.fetch_add(1, Ordering::SeqCst);
                let _ = stream
                    .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\nContent-Length: 0\r\n\r\n")
                    .await;
                let _ = stream.shutdown().await;
            });
        }
    });
    TlsReceiver {
        port,
        requests,
        certificate_pem,
    }
}

fn tls_sender(host: &str, receiver: &TlsReceiver, trust_certificate: bool) -> HttpWebhookSender {
    let resolver = ScriptedResolver::new(vec![Ok(vec![SocketAddr::new(
        "127.0.0.1".parse().unwrap(),
        receiver.port,
    )])]);
    let sender = HttpWebhookSender::from_values_allowing_insecure_loopback(
        &format!("https://{host}:{}/hook", receiver.port),
        SECRET,
    )
    .with_settings(fast_settings())
    .with_resolver(resolver);
    if trust_certificate {
        sender.with_extra_root_certificate_pem(receiver.certificate_pem.as_bytes())
    } else {
        sender
    }
}

#[tokio::test]
async fn tls_certificate_is_verified_against_the_original_host_name_on_the_pinned_address() {
    let receiver = start_tls_receiver("hooks.example.com").await;

    // 名前 (hooks.example.com) は検査済みの 127.0.0.1 に固定して接続し、証明書は元の名前で検証される
    assert_eq!(
        tls_sender("hooks.example.com", &receiver, true)
            .send(&request("1", 1))
            .await,
        WebhookSendObservation::Response {
            status: 204,
            retry_after: None
        }
    );
    assert_eq!(receiver.requests.load(Ordering::SeqCst), 1);

    // 証明書の名前と違うホスト名では、同じ固定先でも検証に失敗する (リクエストは届かない)
    assert_eq!(
        tls_sender("other.example.com", &receiver, true)
            .send(&request("1", 1))
            .await,
        WebhookSendObservation::ConnectFailed
    );
    // 信頼されていない証明書も失敗する
    assert_eq!(
        tls_sender("hooks.example.com", &receiver, false)
            .send(&request("1", 1))
            .await,
        WebhookSendObservation::ConnectFailed
    );
    assert_eq!(receiver.requests.load(Ordering::SeqCst), 1);
}
