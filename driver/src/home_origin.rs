//! Exact home-origin allowlist and reusable pinned, redirect-free HTTP target construction.
use crate::profile_delivery::target::{
    build_client, resolve_target, ProfileDeliveryNetworkSettings, TargetMode,
};
use crate::webhook::{is_forbidden_domain, is_forbidden_ip, HostResolver, SystemResolver};
use error_stack::Report;
use kernel::interfaces::home_link::HomeOriginPolicy;
use kernel::KernelError;
use std::collections::BTreeSet;
use std::sync::Arc;
use url::{Host, Url};

pub const ALLOWED_ORIGINS_ENV: &str = "POST_RELAY_ALLOWED_ORIGINS";
#[derive(Clone)]
pub struct HomeOriginAllowlist {
    origins: BTreeSet<String>,
    resolver: Arc<dyn HostResolver>,
}
impl HomeOriginAllowlist {
    pub fn from_env() -> error_stack::Result<Self, KernelError> {
        match dotenvy::var(ALLOWED_ORIGINS_ENV) {
            Ok(raw) => Self::from_json(&raw),
            Err(dotenvy::Error::EnvVar(std::env::VarError::NotPresent)) => Self::from_json("[]"),
            Err(_) => Err(Report::new(KernelError::Validation)
                .attach_printable("invalid POST_RELAY_ALLOWED_ORIGINS")),
        }
    }
    pub fn from_json(raw: &str) -> error_stack::Result<Self, KernelError> {
        let values: Vec<String> = serde_json::from_str(raw).map_err(|_| {
            Report::new(KernelError::Validation)
                .attach_printable("POST_RELAY_ALLOWED_ORIGINS must be a JSON string array")
        })?;
        let origins = values
            .iter()
            .map(|s| normalize_origin(s))
            .collect::<error_stack::Result<BTreeSet<_>, _>>()?;
        Ok(Self {
            origins,
            resolver: Arc::new(SystemResolver),
        })
    }
    pub fn with_resolver(mut self, resolver: Arc<dyn HostResolver>) -> Self {
        self.resolver = resolver;
        self
    }
    /// S1 uses this again at actual send time. No redirects, proxy bypass or DNS re-resolution.
    /// L1 registration does not make an HTTP probe (no arbitrary external requests).
    pub async fn prepare_operation(
        &self,
        raw: &str,
    ) -> error_stack::Result<PreparedHomeTarget, KernelError> {
        let origin = normalize_origin(raw)?;
        if !self.origins.contains(&origin) {
            return Err(
                Report::new(KernelError::Validation).attach_printable("home origin is not allowed")
            );
        }
        let endpoint = format!("{origin}/internal/v1/emumet/activities");
        let settings = ProfileDeliveryNetworkSettings {
            total_timeout: std::time::Duration::from_secs(10),
            ..Default::default()
        };
        let target = resolve_target(&endpoint, TargetMode::Strict, &self.resolver, &settings)
            .await
            .map_err(|_| {
                Report::new(KernelError::Validation)
                    .attach_printable("home origin has no eligible public target")
            })?;
        let client = build_client(&target, TargetMode::Strict, &settings)
            .ok_or_else(|| Report::new(KernelError::Internal))?;
        Ok(PreparedHomeTarget {
            origin,
            endpoint: target.url,
            client,
        })
    }
}
pub struct PreparedHomeTarget {
    pub origin: String,
    pub endpoint: Url,
    pub client: reqwest::Client,
}
impl HomeOriginPolicy for HomeOriginAllowlist {
    async fn validate(&self, origin: &str) -> error_stack::Result<String, KernelError> {
        Ok(self.prepare_operation(origin).await?.origin)
    }
}
fn normalize_origin(raw: &str) -> error_stack::Result<String, KernelError> {
    let invalid = || {
        Report::new(KernelError::Validation).attach_printable("home must be a public HTTPS origin")
    };
    if raw.trim() != raw || raw.contains(['\\', '*', '@']) {
        return Err(invalid());
    }
    let authority = raw.split_once("://").ok_or_else(invalid)?.1;
    let suffix = authority
        .find(['/', '?', '#'])
        .map(|i| &authority[i..])
        .unwrap_or("");
    if !matches!(suffix, "" | "/") {
        return Err(invalid());
    }
    let url = Url::parse(raw).map_err(|_| invalid())?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid());
    }
    match url.host().ok_or_else(invalid)? {
        Host::Domain(d) if d.ends_with('.') || is_forbidden_domain(d) => return Err(invalid()),
        Host::Ipv4(ip) if is_forbidden_ip(ip.into()) => return Err(invalid()),
        Host::Ipv6(ip) if is_forbidden_ip(ip.into()) => return Err(invalid()),
        _ => {}
    }
    Ok(url.origin().ascii_serialization())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn origin_normalization_and_rejection() {
        assert_eq!(
            normalize_origin("HTTPS://HOME.EXAMPLE:443/").unwrap(),
            "https://home.example"
        );
        for bad in [
            "http://home.example",
            "https://u@home.example",
            "https://@home.example",
            "https://home.example/a/..",
            "https://home.example/?x",
            "https://home.example/#x",
            "https://*.example",
            "https://127.0.0.1",
            "https://[::1]",
            "https://home.example.",
            "https://home.example/a",
            " https://home.example",
        ] {
            assert!(normalize_origin(bad).is_err(), "{bad}");
        }
        for bad in ["", "{}", "[1]", "[\"http://home.example\"]"] {
            assert!(HomeOriginAllowlist::from_json(bad).is_err());
        }
    }
}

#[cfg(test)]
mod dns_tests {
    use super::*;
    use std::net::SocketAddr;
    use std::sync::Mutex;
    struct Dns(Mutex<Vec<SocketAddr>>);
    impl HostResolver for Dns {
        fn resolve<'a>(
            &'a self,
            _: &'a str,
            _: u16,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = std::io::Result<Vec<SocketAddr>>> + Send + 'a>,
        > {
            Box::pin(async move { Ok(self.0.lock().unwrap().clone()) })
        }
    }
    #[tokio::test]
    async fn home_origin_revalidates_allowlist_and_every_dns_answer_for_each_operation() {
        let dns = Arc::new(Dns(Mutex::new(vec!["93.184.216.34:443".parse().unwrap()])));
        let allowed = HomeOriginAllowlist::from_json(r#"["https://home.example:443/"]"#)
            .unwrap()
            .with_resolver(dns.clone());
        let prepared = allowed
            .prepare_operation("https://HOME.EXAMPLE")
            .await
            .unwrap();
        assert_eq!(prepared.origin, "https://home.example");
        assert_eq!(
            prepared.endpoint.as_str(),
            "https://home.example/internal/v1/emumet/activities"
        );
        assert!(allowed.validate("https://sub.home.example").await.is_err());
        // A successful registration must not cache trust across a DNS-rebinding change.
        dns.0.lock().unwrap().push("127.0.0.1:443".parse().unwrap());
        assert!(allowed
            .prepare_operation("https://home.example")
            .await
            .is_err());
        *dns.0.lock().unwrap() = vec!["[::1]:443".parse().unwrap()];
        assert!(allowed.validate("https://home.example").await.is_err());
        dns.0.lock().unwrap().clear();
        assert!(allowed.validate("https://home.example").await.is_err());
        // Removing an origin is a per-operation hold, not an unlink/mutation callback.
        assert!(HomeOriginAllowlist::from_json("[]")
            .unwrap()
            .validate("https://home.example")
            .await
            .is_err());
    }
}
