use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use error_stack::{Report, Result};
use kernel::KernelError;
use reqwest::header::{ACCEPT, LOCATION, USER_AGENT};
use serde_json::Value;

use super::ssrf::validate_fetch_url;

pub(super) async fn fetch_limited_json(
    client: &reqwest::Client,
    max_response_bytes: usize,
    mut url: reqwest::Url,
) -> Result<Value, KernelError> {
    for _ in 0..=5 {
        let resolved_addresses = validate_fetch_url(&url).await?;

        let response = client_for_url(client, &url, &resolved_addresses)?
            .get(url.clone())
            .header(ACCEPT, "application/activity+json")
            .header(USER_AGENT, "Emumet/0.1 ActivityPub HTTP Signature verifier")
            .send()
            .await
            .map_err(|e| {
                Report::new(KernelError::Rejected)
                    .attach_printable(format!("KeyFetchFailed: actor key fetch failed: {e}"))
            })?;

        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| {
                    Report::new(KernelError::Rejected)
                        .attach_printable("KeyFetchFailed: redirect without Location header")
                })?;
            url = url.join(location).map_err(|e| {
                Report::new(KernelError::Rejected)
                    .attach_printable(format!("KeyFetchFailed: malformed redirect URL: {e}"))
            })?;
            continue;
        }

        if !response.status().is_success() {
            let status = response.status();
            let body_text = response.text().await.unwrap_or_default();
            tracing::debug!(
                key_fetch_url = %url,
                key_fetch_status = %status,
                key_fetch_body = %body_text,
                "KeyFetch failed with non-success status"
            );
            return Err(Report::new(KernelError::Rejected).attach_printable(format!(
                "KeyFetchFailed: actor key endpoint returned {}",
                status,
            )));
        }

        let mut bytes = Vec::new();
        let mut response = response;
        while let Some(chunk) = response.chunk().await.map_err(|e| {
            Report::new(KernelError::Rejected)
                .attach_printable(format!("KeyFetchFailed: response read failed: {e}"))
        })? {
            if bytes.len() + chunk.len() > max_response_bytes {
                return Err(Report::new(KernelError::Rejected)
                    .attach_printable("KeyFetchFailed: actor key response exceeds 1 MiB"));
            }
            bytes.extend_from_slice(&chunk);
        }

        return serde_json::from_slice(&bytes).map_err(|e| {
            Report::new(KernelError::Rejected)
                .attach_printable(format!("KeyFetchFailed: invalid actor JSON: {e}"))
        });
    }

    Err(Report::new(KernelError::Rejected)
        .attach_printable("KeyFetchFailed: too many redirects while fetching actor key"))
}

fn client_for_url(
    client: &reqwest::Client,
    url: &reqwest::Url,
    resolved_addresses: &[SocketAddr],
) -> Result<reqwest::Client, KernelError> {
    let Some(host) = url.host_str() else {
        return Ok(client.clone());
    };

    if host.parse::<IpAddr>().is_ok() {
        return Ok(client.clone());
    }

    let builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10));
    #[cfg(any(test, feature = "test-mode"))]
    let builder = if std::env::var("AP_TEST_ACCEPT_INVALID_CERTS").as_deref() == Ok("1") {
        builder.danger_accept_invalid_certs(true)
    } else {
        builder
    };
    builder
        .resolve_to_addrs(host, resolved_addresses)
        .build()
        .map_err(|e| {
            Report::new(KernelError::Internal)
                .attach_printable(format!("Failed to build pinned HTTP client: {e}"))
        })
}
