//! Webhook endpoint 設定の解釈と宛先検査 (SSRF 対策)。
//!
//! endpoint URL と署名 secret はサーバー設定 (環境変数/secret ファイル) だけで管理する。
//! 設定不備は起動失敗にせず `WebhookConfigIssue` として状態 API に出す。secret・URL の値は
//! Debug/ログ/エラーに出さない。

use kernel::prelude::entity::WebhookConfigIssue;
use reqwest::Url;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use url::Host;
use zeroize::Zeroize;

pub const ENV_URL: &str = "REGISTRATION_WEBHOOK_URL";
pub const ENV_SECRET: &str = "REGISTRATION_WEBHOOK_SECRET";
pub const ENV_SECRET_FILE: &str = "REGISTRATION_WEBHOOK_SECRET_FILE";

/// 署名 secret の最小長 (バイト)。
pub const MIN_SECRET_BYTES: usize = 16;

/// 宛先の厳格さ。production 設定は常に `Strict`。`AllowInsecureLoopback` は
/// `cfg(test)` / `test-mode` feature のビルドにだけ存在し、環境変数など設定からは選べない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EndpointMode {
    Strict,
    #[cfg(any(test, feature = "test-mode"))]
    AllowInsecureLoopback,
}

impl EndpointMode {
    pub(crate) fn is_strict(self) -> bool {
        matches!(self, EndpointMode::Strict)
    }
}

/// 署名 secret。Debug に値を出さず、drop 時にゼロ埋めする。
#[derive(Clone)]
pub struct WebhookSecret(Vec<u8>);

impl WebhookSecret {
    pub(crate) fn new(bytes: Vec<u8>) -> Result<Self, WebhookConfigIssue> {
        if bytes.len() < MIN_SECRET_BYTES {
            return Err(WebhookConfigIssue::SecretTooShort);
        }
        Ok(Self(bytes))
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl std::fmt::Debug for WebhookSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WebhookSecret(<redacted>)")
    }
}

impl Drop for WebhookSecret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EndpointHost {
    Domain(String),
    Ip(IpAddr),
}

/// 検査済みの endpoint。
#[derive(Debug, Clone)]
pub(crate) struct WebhookEndpoint {
    pub url: Url,
    pub host: EndpointHost,
    pub port: u16,
}

impl WebhookEndpoint {
    /// 状態 API に出す宛先 (host 部分のみ。userinfo・port・path・query は出さない)。
    pub(crate) fn display_host(&self) -> String {
        match &self.host {
            EndpointHost::Domain(domain) => domain.clone(),
            EndpointHost::Ip(IpAddr::V4(ip)) => ip.to_string(),
            EndpointHost::Ip(IpAddr::V6(ip)) => format!("[{ip}]"),
        }
    }
}

/// webhook 宛先として拒否する IP。既存の AP 取得用検査 (`http_signing::ssrf`) に加え、
/// IPv4-mapped IPv6・NAT64・IPv4 互換・site-local など「グローバル unicast ではない」IPv6 を
/// すべて拒否する (許可するのは 2000::/3 のうち既存の拒否範囲を除いたものだけ)。
pub(crate) fn is_forbidden_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_forbidden_ipv4(ip),
        IpAddr::V6(ip) => is_forbidden_ipv6(ip),
    }
}

fn is_forbidden_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    crate::http_signing::ssrf::is_blocked_ipv4(ip)
        || (octets[0] == 192 && octets[1] == 88 && octets[2] == 99)
}

fn is_forbidden_ipv6(ip: Ipv6Addr) -> bool {
    if ip.to_ipv4_mapped().is_some() {
        return true;
    }
    let global_unicast = (ip.segments()[0] & 0xe000) == 0x2000;
    !global_unicast || crate::http_signing::ssrf::is_blocked_ipv6(ip)
}

fn is_forbidden_domain(domain: &str) -> bool {
    domain == "localhost"
        || domain.ends_with(".localhost")
        || domain == "metadata.google.internal"
        || domain.ends_with(".metadata.google.internal")
}

/// endpoint URL を検査して解釈する。
///
/// - 解釈できない URL (IPv6 zone 付きなど)・https 以外・userinfo 付き・host なしは拒否
/// - `localhost` 系ドメインと、リテラル IP が内部アドレスなら拒否 (`ForbiddenDestination`)
/// - 10 進/16 進/短縮 IPv4 などの表記ゆれは URL パーサが正規化した IPv4 として検査される
/// - 末尾ドット付き host は末尾ドットを除いて検査・接続する
///
/// ドメインの DNS 解決後の検査は送信時に毎回行う (`HttpWebhookSender`)。
pub(crate) fn parse_endpoint(
    raw: &str,
    mode: EndpointMode,
) -> Result<WebhookEndpoint, WebhookConfigIssue> {
    let mut url = Url::parse(raw.trim()).map_err(|_| WebhookConfigIssue::InvalidUrl)?;
    match url.scheme() {
        "https" => {}
        #[cfg(any(test, feature = "test-mode"))]
        "http" if !mode.is_strict() => {}
        _ => return Err(WebhookConfigIssue::UnsupportedScheme),
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(WebhookConfigIssue::CredentialsInUrl);
    }
    let port = url
        .port_or_known_default()
        .ok_or(WebhookConfigIssue::InvalidUrl)?;
    let host = match url.host() {
        None => return Err(WebhookConfigIssue::MissingHost),
        Some(Host::Ipv4(ip)) => EndpointHost::Ip(IpAddr::V4(ip)),
        Some(Host::Ipv6(ip)) => EndpointHost::Ip(IpAddr::V6(ip)),
        Some(Host::Domain(domain)) => {
            let normalized = domain.trim_end_matches('.').to_ascii_lowercase();
            if normalized.is_empty() {
                return Err(WebhookConfigIssue::MissingHost);
            }
            if normalized != domain {
                url.set_host(Some(&normalized))
                    .map_err(|_| WebhookConfigIssue::InvalidUrl)?;
            }
            EndpointHost::Domain(normalized)
        }
    };
    if mode.is_strict() {
        let forbidden = match &host {
            EndpointHost::Domain(domain) => is_forbidden_domain(domain),
            EndpointHost::Ip(ip) => is_forbidden_ip(*ip),
        };
        if forbidden {
            return Err(WebhookConfigIssue::ForbiddenDestination);
        }
    }
    Ok(WebhookEndpoint { url, host, port })
}

/// 環境変数の値 (またはテスト入力) から secret を取り出す。直接指定とファイル指定は排他。
pub(crate) fn load_secret(
    direct: Option<&str>,
    file: Option<&str>,
) -> Result<Option<WebhookSecret>, WebhookConfigIssue> {
    let direct = direct.filter(|value| !value.trim().is_empty());
    let file = file.filter(|value| !value.trim().is_empty());
    let bytes = match (direct, file) {
        (Some(_), Some(_)) => return Err(WebhookConfigIssue::AmbiguousSecretSource),
        (None, None) => return Ok(None),
        (Some(value), None) => value.trim().as_bytes().to_vec(),
        (None, Some(path)) => {
            let mut content =
                std::fs::read_to_string(path).map_err(|_| WebhookConfigIssue::SecretUnreadable)?;
            let trimmed = content.trim().as_bytes().to_vec();
            content.zeroize();
            if trimmed.is_empty() {
                return Err(WebhookConfigIssue::SecretUnreadable);
            }
            trimmed
        }
    };
    WebhookSecret::new(bytes).map(Some)
}
