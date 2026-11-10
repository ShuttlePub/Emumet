use crate::entity::{GraphActorUrl, GraphInboxUrl, ProfileSendObservation};
use crate::KernelError;
use error_stack::Report;
use std::future::Future;

/// 署名済みの 1 件の配送 request。`headers` は署名ヘッダーを含む全ヘッダー (`Host` は URL から
/// HTTP クライアントが設定するので含めない)。`body` は job に保存されたバイト列そのもので、
/// `Digest` はこのバイト列から計算済み。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileDeliveryRequest<'a> {
    pub url: &'a str,
    pub headers: &'a [(String, String)],
    pub body: &'a [u8],
}

/// 外部 inbox への送信 port (P3o の配送 worker が使う)。宛先検査 (SSRF 対策)・名前解決の固定・
/// timeout・redirect の拒否は実装側の責務。署名は呼び出し側 (application の Profile 署名 use case) が行う。
pub trait ProfileDeliverySender: Sync + Send + 'static {
    fn send(
        &self,
        request: &ProfileDeliveryRequest<'_>,
    ) -> impl Future<Output = ProfileSendObservation> + Send;
}

pub trait DependOnProfileDeliverySender: Sync + Send {
    type ProfileDeliverySender: ProfileDeliverySender;

    fn profile_delivery_sender(&self) -> &Self::ProfileDeliverySender;
}

/// 解決された remote actor。`actor_url` は actor 文書の `id` (canonical)、`inbox` は配送先。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteActorInfo {
    pub actor_url: GraphActorUrl,
    pub inbox: GraphInboxUrl,
}

/// `acct:name@domain` 形式の handle (WebFinger の resource)。`acct:` と先頭の `@` は省略できる。
/// domain は小文字化され、port 付き (`host:port`) も許す。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcctHandle {
    name: String,
    domain: String,
}

impl AcctHandle {
    pub fn parse(raw: &str) -> error_stack::Result<Self, KernelError> {
        let invalid = |reason: &str| {
            Report::new(KernelError::Validation).attach_printable(format!("invalid acct: {reason}"))
        };
        let rest = raw.strip_prefix("acct:").unwrap_or(raw);
        let rest = rest.strip_prefix('@').unwrap_or(rest);
        let (name, domain) = rest.rsplit_once('@').ok_or_else(|| invalid("missing @"))?;
        let name_ok = !name.is_empty()
            && name.len() <= 256
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
        let (host, port) = match domain.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (domain, None),
        };
        let host_ok = !host.is_empty()
            && host.len() <= 253
            && !host.starts_with(['.', '-'])
            && !host.ends_with(['.', '-'])
            && host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'));
        let port_ok = port.is_none_or(|port| {
            !port.is_empty() && port.len() <= 5 && port.chars().all(|c| c.is_ascii_digit())
        });
        if !name_ok || !host_ok || !port_ok {
            return Err(invalid("malformed name or domain"));
        }
        Ok(Self {
            name: name.to_string(),
            domain: domain.to_ascii_lowercase(),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn domain(&self) -> &str {
        &self.domain
    }

    /// WebFinger の `resource` 値 (`acct:name@domain`)。
    pub fn resource(&self) -> String {
        format!("acct:{}@{}", self.name, self.domain)
    }
}

/// remote actor の actor URL から inbox を解決する port。Follow の宛先 inbox の取得に使う。
///
/// 失敗は `Rejected` (不正・許可されない宛先、`id` の不一致、inbox の欠落)、`NotFound` (404/410)、
/// `Timeout`、`Internal`。解決は関係変更の transaction の外で行う。
pub trait RemoteActorResolver: Sync + Send + 'static {
    /// WebFinger (`/.well-known/webfinger?resource=acct:name@domain`) で handle を canonical な actor URL に
    /// 解決する。宛先検査は `resolve` と同じ。失敗は `Rejected` / `NotFound` / `Timeout`。
    fn resolve_acct(
        &self,
        handle: &AcctHandle,
    ) -> impl Future<Output = error_stack::Result<GraphActorUrl, KernelError>> + Send;

    fn resolve(
        &self,
        actor_url: &GraphActorUrl,
    ) -> impl Future<Output = error_stack::Result<RemoteActorInfo, KernelError>> + Send;
}

pub trait DependOnRemoteActorResolver: Sync + Send {
    type RemoteActorResolver: RemoteActorResolver;

    fn remote_actor_resolver(&self) -> &Self::RemoteActorResolver;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acct_handles_parse_with_or_without_prefixes() {
        for raw in [
            "acct:bob@Remote.Example",
            "@bob@remote.example",
            "bob@remote.example",
        ] {
            let handle = AcctHandle::parse(raw).unwrap();
            assert_eq!(handle.name(), "bob");
            assert_eq!(handle.domain(), "remote.example");
            assert_eq!(handle.resource(), "acct:bob@remote.example");
        }
        assert_eq!(
            AcctHandle::parse("bob@127.0.0.1:8080").unwrap().domain(),
            "127.0.0.1:8080"
        );
    }

    #[test]
    fn malformed_handles_are_rejected() {
        for raw in [
            "",
            "bob",
            "@",
            "bob@",
            "@remote.example",
            "bob@remote.example.",
            "bob@remote..example/x",
            "bob@remote.example:port",
            "bob smith@remote.example",
            "bob@-remote.example",
        ] {
            assert!(AcctHandle::parse(raw).is_err(), "{raw}");
        }
    }
}
