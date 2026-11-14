use super::target::{
    build_client, read_limited, resolve_target, ProfileDeliveryNetworkSettings, TargetError,
    TargetMode,
};
use crate::webhook::{HostResolver, SystemResolver};
use error_stack::Report;
use kernel::interfaces::repository::{
    AcctHandle, RemoteActorInfo, RemoteActorProfile, RemoteActorResolver,
};
use kernel::prelude::entity::{GraphActorUrl, GraphInboxUrl};
use kernel::KernelError;
use reqwest::header::ACCEPT;
use std::sync::Arc;

/// Test-mode only: remote actors whose documents cannot be fetched without authentication (for
/// example Iceshrimp with authorized fetch). `resolve` answers from this cache before any request.
#[cfg(any(test, feature = "test-mode"))]
static TEST_RESOLVED_ACTORS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, String>>,
> = std::sync::LazyLock::new(Default::default);

/// Registers the inbox of a remote actor in the test cache used by [`HttpRemoteActorResolver`].
#[cfg(any(test, feature = "test-mode"))]
pub fn inject_test_resolved_actor(actor_url: &str, inbox_url: &str) {
    TEST_RESOLVED_ACTORS
        .lock()
        .expect("poisoned lock")
        .insert(actor_url.to_string(), inbox_url.to_string());
}

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

impl HttpRemoteActorResolver {
    /// actor 文書を取得して JSON として返す。文書の `id` が要求した URL と完全一致することまで検査する。
    async fn fetch_document(
        &self,
        actor_url: &GraphActorUrl,
    ) -> error_stack::Result<serde_json::Value, KernelError> {
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
        Ok(document)
    }
}

impl RemoteActorResolver for HttpRemoteActorResolver {
    async fn resolve_acct(
        &self,
        handle: &AcctHandle,
    ) -> error_stack::Result<GraphActorUrl, KernelError> {
        let scheme = if self.mode.is_strict() {
            "https"
        } else {
            "http"
        };
        let mut url = reqwest::Url::parse(&format!(
            "{scheme}://{}/.well-known/webfinger",
            handle.domain()
        ))
        .map_err(|_| rejected("acct domain is not a valid host"))?;
        let resource = handle.resource();
        url.query_pairs_mut().append_pair("resource", &resource);
        let target = resolve_target(url.as_str(), self.mode, &self.resolver, &self.settings)
            .await
            .map_err(|error| match error {
                TargetError::Invalid => rejected("acct domain is not an allowed destination"),
                TargetError::DnsFailed => Report::new(KernelError::Timeout)
                    .attach_printable("acct domain could not be resolved"),
            })?;
        let client = build_client(&target, self.mode, &self.settings)
            .ok_or_else(|| Report::new(KernelError::Internal))?;
        let mut response = client
            .get(target.url.clone())
            .header(ACCEPT, "application/jrd+json, application/json")
            .send()
            .await
            .map_err(|error| {
                if error.is_timeout() {
                    Report::new(KernelError::Timeout)
                } else {
                    rejected("webfinger request failed to connect")
                }
            })?;
        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::GONE {
            return Err(Report::new(KernelError::NotFound)
                .attach_printable("webfinger does not know the account"));
        }
        if !status.is_success() {
            return Err(rejected("webfinger returned an unsuccessful status"));
        }
        let body = read_limited(&mut response, self.settings.max_response_bytes).await;
        let document: serde_json::Value = serde_json::from_slice(&body)
            .map_err(|_| rejected("webfinger response is not JSON"))?;
        let subject = document
            .get("subject")
            .and_then(|value| value.as_str())
            .filter(|subject| !subject.is_empty())
            .ok_or_else(|| rejected("webfinger response has no subject"))?;
        let subject = subject.strip_prefix("acct:").unwrap_or(subject);
        if !subject.eq_ignore_ascii_case(resource.strip_prefix("acct:").unwrap_or(&resource)) {
            return Err(rejected(
                "webfinger subject does not match the requested account",
            ));
        }
        let href = document
            .get("links")
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
            .find(|link| {
                link.get("rel").and_then(|v| v.as_str()) == Some("self")
                    && link.get("type").and_then(|v| v.as_str()).is_some_and(|t| {
                        t.starts_with("application/activity+json")
                            || t.starts_with("application/ld+json")
                    })
            })
            .and_then(|link| link.get("href"))
            .and_then(|value| value.as_str())
            .ok_or_else(|| rejected("webfinger has no ActivityPub self link"))?;
        GraphActorUrl::new(href).map_err(|_| rejected("webfinger self link is not an actor URL"))
    }

    async fn resolve(
        &self,
        actor_url: &GraphActorUrl,
    ) -> error_stack::Result<RemoteActorInfo, KernelError> {
        #[cfg(any(test, feature = "test-mode"))]
        {
            let cached = TEST_RESOLVED_ACTORS
                .lock()
                .expect("poisoned lock")
                .get(actor_url.as_str())
                .cloned();
            if let Some(inbox) = cached {
                let inbox = GraphInboxUrl::new(inbox)
                    .map_err(|_| rejected("cached actor inbox is not a URL"))?;
                return Ok(RemoteActorInfo {
                    actor_url: actor_url.clone(),
                    inbox,
                });
            }
        }
        let document = self.fetch_document(actor_url).await?;
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

    async fn fetch_profile(
        &self,
        actor_url: &GraphActorUrl,
    ) -> error_stack::Result<RemoteActorProfile, KernelError> {
        let document = self.fetch_document(actor_url).await?;
        RemoteActorProfile::from_actor_document(&document)
    }
}
