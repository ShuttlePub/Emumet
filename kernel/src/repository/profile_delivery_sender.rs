use crate::entity::{GraphActorUrl, GraphInboxUrl, ProfileSendObservation};
use crate::KernelError;
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

/// remote actor の actor URL から inbox を解決する port。Follow の宛先 inbox の取得に使う。
///
/// 失敗は `Rejected` (不正・許可されない宛先、`id` の不一致、inbox の欠落)、`NotFound` (404/410)、
/// `Timeout`、`Internal`。解決は関係変更の transaction の外で行う。
pub trait RemoteActorResolver: Sync + Send + 'static {
    fn resolve(
        &self,
        actor_url: &GraphActorUrl,
    ) -> impl Future<Output = error_stack::Result<RemoteActorInfo, KernelError>> + Send;
}

pub trait DependOnRemoteActorResolver: Sync + Send {
    type RemoteActorResolver: RemoteActorResolver;

    fn remote_actor_resolver(&self) -> &Self::RemoteActorResolver;
}
