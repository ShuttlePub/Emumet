use super::target::{
    build_client, read_limited, resolve_target, ProfileDeliveryNetworkSettings, TargetError,
    TargetMode,
};
use crate::webhook::{HostResolver, SystemResolver};
use error_stack::Report;
use kernel::interfaces::repository::{RemoteActorInfo, RemoteActorResolver};
use kernel::prelude::entity::{GraphActorUrl, GraphInboxUrl};
use kernel::KernelError;
use reqwest::header::ACCEPT;
use std::sync::Arc;

const ACTOR_ACCEPT: &str = "application/activity+json, application/ld+json; profile=\"https://www.w3.org/ns/activitystreams\"";

/// remote actor の actor 文書を取得して inbox を解決する。
///
/// 取得は署名なしの GET。actor 文書の `id` が要求した actor URL と完全一致することを要求する
/// (別 actor の文書を指す URL や redirect は拒否)。
#[derive(Clone)]
pub struct HttpRemoteActorResolver {
    mode: TargetMode,
    settings: ProfileDeliveryNetworkSettings,
    resolver: Arc<dyn HostResolver>,
}

impl HttpRemoteActorResolver {
    pub fn new() -> Self {
        Self {
            mode: TargetMode::Strict,
            settings: ProfileDeliveryNetworkSettings::default(),
            resolver: Arc::new(SystemResolver),
        }
    }

    /// http・loopback 宛先を許可する検証用の構築。`cfg(test)` / `test-mode` ビルドにだけ存在する。
    #[cfg(any(test, feature = "test-mode"))]
    pub fn allowing_insecure_loopback() -> Self {
        Self {
            mode: TargetMode::AllowInsecureLoopback,
            ..Self::new()
        }
    }

    pub fn with_settings(mut self, settings: ProfileDeliveryNetworkSettings) -> Self {
        self.settings = settings;
        self
    }

    pub fn with_resolver(mut self, resolver: Arc<dyn HostResolver>) -> Self {
        self.resolver = resolver;
        self
    }
}

impl Default for HttpRemoteActorResolver {
    fn default() -> Self {
        Self::new()
    }
}

fn rejected(message: &str) -> Report<KernelError> {
    Report::new(KernelError::Rejected).attach_printable(message.to_string())
}

impl RemoteActorResolver for HttpRemoteActorResolver {
    async fn resolve(
        &self,
        actor_url: &GraphActorUrl,
    ) -> error_stack::Result<RemoteActorInfo, KernelError> {
        let target = resolve_target(
            actor_url.as_str(),
            self.mode,
            &self.resolver,
            &self.settings,
        )
        .await
        .map_err(|error| match error {
            TargetError::Invalid => rejected("actor URL is not an allowed destination"),
            TargetError::DnsFailed => Report::new(KernelError::Timeout)
                .attach_printable("actor host could not be resolved"),
        })?;
        let client = build_client(&target, self.mode, &self.settings)
            .ok_or_else(|| Report::new(KernelError::Internal))?;
        let mut response = client
            .get(target.url.clone())
            .header(ACCEPT, ACTOR_ACCEPT)
            .send()
            .await
            .map_err(|error| {
                if error.is_timeout() {
                    Report::new(KernelError::Timeout)
                } else {
                    rejected("actor fetch failed to connect")
                }
            })?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::GONE {
            return Err(
                Report::new(KernelError::NotFound).attach_printable("remote actor does not exist")
            );
        }
        if !status.is_success() {
            return Err(rejected("actor fetch returned an unsuccessful status"));
        }
        let body = read_limited(&mut response, self.settings.max_response_bytes).await;
        let document: serde_json::Value =
            serde_json::from_slice(&body).map_err(|_| rejected("actor document is not JSON"))?;
        let id = document
            .get("id")
            .and_then(|value| value.as_str())
            .ok_or_else(|| rejected("actor document has no id"))?;
        if id != actor_url.as_str() {
            return Err(rejected(
                "actor document id does not match the requested URL",
            ));
        }
        let inbox = document
            .get("inbox")
            .and_then(|value| value.as_str())
            .ok_or_else(|| rejected("actor document has no inbox"))?;
        let inbox = GraphInboxUrl::new(inbox).map_err(|_| rejected("actor inbox is not a URL"))?;
        Ok(RemoteActorInfo {
            actor_url: actor_url.clone(),
            inbox,
        })
    }
}
