use crate::webhook::{is_forbidden_domain, is_forbidden_ip, HostResolver};
use reqwest::Url;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use url::Host;

/// 送信の時間・サイズの上限。
#[derive(Debug, Clone, Copy)]
pub struct ProfileDeliveryNetworkSettings {
    pub resolve_timeout: Duration,
    pub connect_timeout: Duration,
    /// 接続から応答の受信完了までの全体 timeout。worker の lease はこれより十分長くする。
    pub total_timeout: Duration,
    /// 応答本文として読み捨てる (または actor 文書として読む) 最大バイト数。
    pub max_response_bytes: usize,
}

impl Default for ProfileDeliveryNetworkSettings {
    fn default() -> Self {
        Self {
            resolve_timeout: Duration::from_secs(5),
            connect_timeout: Duration::from_secs(10),
            total_timeout: Duration::from_secs(30),
            max_response_bytes: 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TargetMode {
    Strict,
    #[cfg(any(test, feature = "test-mode"))]
    AllowInsecureLoopback,
}

impl TargetMode {
    pub(crate) fn is_strict(self) -> bool {
        matches!(self, TargetMode::Strict)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TargetError {
    /// 解釈できない・許可されない宛先 (送信していない)。
    Invalid,
    DnsFailed,
}

/// 検査済みで、接続先アドレスを固定した宛先。
#[derive(Debug, Clone)]
pub(crate) struct PinnedTarget {
    pub url: Url,
    domain: Option<String>,
    pub addresses: Vec<SocketAddr>,
}

pub(crate) async fn resolve_target(
    raw: &str,
    mode: TargetMode,
    resolver: &Arc<dyn HostResolver>,
    settings: &ProfileDeliveryNetworkSettings,
) -> Result<PinnedTarget, TargetError> {
    let url = Url::parse(raw).map_err(|_| TargetError::Invalid)?;
    match url.scheme() {
        "https" => {}
        #[cfg(any(test, feature = "test-mode"))]
        "http" if !mode.is_strict() => {}
        _ => return Err(TargetError::Invalid),
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(TargetError::Invalid);
    }
    let port = url.port_or_known_default().ok_or(TargetError::Invalid)?;
    let (domain, addresses) = match url.host().ok_or(TargetError::Invalid)? {
        Host::Ipv4(ip) => (None, vec![SocketAddr::new(IpAddr::V4(ip), port)]),
        Host::Ipv6(ip) => (None, vec![SocketAddr::new(IpAddr::V6(ip), port)]),
        Host::Domain(domain) => {
            // 検査・名前解決・接続先の固定 (DNS override) は、URL がそのまま使う host 文字列と完全に
            // 一致しなければならない。末尾ドット付き (`example.com.`) は、検査した名前と接続時の名前が
            // ずれて DNS override が効かず、通常の DNS 解決で内部アドレスへ接続されうるので拒否する。
            if domain.is_empty() || domain.ends_with('.') {
                return Err(TargetError::Invalid);
            }
            let domain = domain.to_string();
            if mode.is_strict() && is_forbidden_domain(&domain) {
                return Err(TargetError::Invalid);
            }
            let resolved =
                tokio::time::timeout(settings.resolve_timeout, resolver.resolve(&domain, port))
                    .await;
            match resolved {
                Ok(Ok(addresses)) if !addresses.is_empty() => (Some(domain), addresses),
                _ => return Err(TargetError::DnsFailed),
            }
        }
    };
    if mode.is_strict() && addresses.iter().any(|a| is_forbidden_ip(a.ip())) {
        return Err(TargetError::Invalid);
    }
    Ok(PinnedTarget {
        url,
        domain,
        addresses,
    })
}

pub(crate) fn build_client(
    target: &PinnedTarget,
    mode: TargetMode,
    settings: &ProfileDeliveryNetworkSettings,
) -> Option<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .connect_timeout(settings.connect_timeout)
        .timeout(settings.total_timeout)
        .pool_max_idle_per_host(0)
        .https_only(mode.is_strict());
    // E2E peers (Iceshrimp / Mastodon behind nginx) use a self-signed certificate. Only the
    // non-strict test construction can ever get here, so production never skips verification.
    #[cfg(any(test, feature = "test-mode"))]
    if !mode.is_strict() && std::env::var("AP_TEST_ACCEPT_INVALID_CERTS").as_deref() == Ok("1") {
        builder = builder.danger_accept_invalid_certs(true);
    }
    if let Some(domain) = &target.domain {
        builder = builder.resolve_to_addrs(domain, &target.addresses);
    }
    builder.build().ok()
}

/// 応答本文を `limit` バイトまで読む (超えた分は捨てる)。
pub(crate) async fn read_limited(response: &mut reqwest::Response, limit: usize) -> Vec<u8> {
    let mut body = Vec::new();
    while body.len() < limit {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let room = limit - body.len();
                body.extend_from_slice(&chunk[..chunk.len().min(room)]);
            }
            _ => break,
        }
    }
    body
}
