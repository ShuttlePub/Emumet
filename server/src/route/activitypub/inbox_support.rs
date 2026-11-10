//! Helpers shared by the Profile inbox: header handling and Host / key-owner comparisons.

use crate::error::ErrorStatus;
use axum::http::{header, HeaderMap, StatusCode};
use std::collections::HashMap;

pub(super) fn headers_to_map(headers: &HeaderMap) -> HashMap<String, String> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_string(), value.to_string()))
        })
        .collect()
}

pub(super) fn ensure_host_matches_public_base_url(
    expected: &str,
    headers: &HeaderMap,
) -> Result<(), ErrorStatus> {
    let actual = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| ErrorStatus::from(StatusCode::UNAUTHORIZED))?;
    if actual.eq_ignore_ascii_case(expected) {
        return Ok(());
    }
    // In test mode, also accept localhost connections (e.g., when the test
    // environment routes ActivityPub traffic directly to localhost:8080
    // instead of through the nginx reverse proxy). HTTP Signature verification
    // remains the primary security guard for inbox requests.
    if cfg!(any(test, feature = "test-mode"))
        && (actual.eq_ignore_ascii_case("localhost")
            || actual.to_ascii_lowercase().starts_with("localhost:"))
    {
        tracing::debug!(
            expected,
            actual,
            "ActivityPub inbox Host is localhost (test-mode override)"
        );
        return Ok(());
    }
    tracing::warn!(
        expected,
        actual,
        "ActivityPub inbox Host does not match PUBLIC_BASE_URL"
    );
    Err(ErrorStatus::from(StatusCode::UNAUTHORIZED))
}

pub(super) fn same_activitypub_id(left: &str, right: &str) -> bool {
    left.trim_end_matches('/') == right.trim_end_matches('/')
}

pub(super) fn signature_key_document_matches_actor(key_id: &str, actor: &str) -> bool {
    let Ok(mut key_url) = url::Url::parse(key_id) else {
        return false;
    };
    key_url.set_fragment(None);
    same_activitypub_id(key_url.as_str(), actor)
}
