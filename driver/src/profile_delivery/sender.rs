use super::target::{
    build_client, read_limited, resolve_target, ProfileDeliveryNetworkSettings, TargetError,
    TargetMode,
};
use crate::webhook::{parse_retry_after, HostResolver, SystemResolver};
use kernel::interfaces::repository::{ProfileDeliveryRequest, ProfileDeliverySender};
use kernel::prelude::entity::ProfileSendObservation;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, RETRY_AFTER};
use std::sync::Arc;
use std::time::SystemTime;

/// 署名済み activity を remote inbox へ POST する sender。
///
/// 署名は呼び出し側が済ませている。ここでは宛先検査・名前解決の固定・redirect 拒否・timeout・
/// 応答の観測 (status と `Retry-After`) だけを行い、再試行の判断はしない。
#[derive(Clone)]
pub struct HttpProfileDeliverySender {
    mode: TargetMode,
    settings: ProfileDeliveryNetworkSettings,
    resolver: Arc<dyn HostResolver>,
}

impl std::fmt::Debug for HttpProfileDeliverySender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpProfileDeliverySender").finish()
    }
}

impl HttpProfileDeliverySender {
    /// production 設定。https のみ、内部宛先は拒否される。
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

impl Default for HttpProfileDeliverySender {
    fn default() -> Self {
        Self::new()
    }
}

fn header_map(headers: &[(String, String)]) -> Option<HeaderMap> {
    let mut map = HeaderMap::new();
    for (name, value) in headers {
        if name.eq_ignore_ascii_case("host") {
            continue;
        }
        let name = HeaderName::from_bytes(name.as_bytes()).ok()?;
        let value = HeaderValue::from_str(value).ok()?;
        map.append(name, value);
    }
    Some(map)
}

impl ProfileDeliverySender for HttpProfileDeliverySender {
    async fn send(&self, request: &ProfileDeliveryRequest<'_>) -> ProfileSendObservation {
        let target =
            match resolve_target(request.url, self.mode, &self.resolver, &self.settings).await {
                Ok(target) => target,
                Err(TargetError::Invalid) => return ProfileSendObservation::InvalidTarget,
                Err(TargetError::DnsFailed) => return ProfileSendObservation::DnsFailed,
            };
        let Some(headers) = header_map(request.headers) else {
            return ProfileSendObservation::InvalidTarget;
        };
        let Some(client) = build_client(&target, self.mode, &self.settings) else {
            return ProfileSendObservation::InvalidTarget;
        };
        let response = client
            .post(target.url.clone())
            .headers(headers)
            .body(request.body.to_vec())
            .send()
            .await;
        let mut response = match response {
            Ok(response) => response,
            Err(error) if error.is_timeout() => return ProfileSendObservation::Timeout,
            Err(_) => return ProfileSendObservation::ConnectFailed,
        };
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| parse_retry_after(value, SystemTime::now()));
        read_limited(&mut response, self.settings.max_response_bytes).await;
        ProfileSendObservation::Response {
            status,
            retry_after,
        }
    }
}
