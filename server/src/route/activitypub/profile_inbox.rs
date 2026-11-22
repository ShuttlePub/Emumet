//! Profile ActivityPub inbox (P3i).
//!
//! `POST /ap/profiles/{profile_nanoid}/inbox` receives signed Follow / Accept / Reject /
//! Undo(Follow) activities for one Profile. It is mounted on the production root router by the P4
//! cutover (replacing the retired `/ap/accounts/{id}/inbox`) through [`ProfileInboxRouter`].
//!
//! The route verifies the HTTP Signature and that the key owner is the `activity.actor`; the
//! relation rules (target Profile, original Follow ID, stale / foreign / duplicate handling) live
//! in `application::service::activitypub::ProfileInboxUseCase`.

use super::inbox_support::{
    ensure_host_matches_public_base_url, headers_to_map, same_activitypub_id,
    signature_key_document_matches_actor,
};
use crate::api::ActivityPubApi;
use crate::error::ErrorStatus;
use crate::handler::AppModule;
use application::inbox_reading::{build_evidence, media_type_ok, read_inbox, REQUEST_TARGET_MAX};
use application::service::activitypub::{InboxOverload, ProfileRelayInboxDto};
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, OriginalUri, Path, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use kernel::interfaces::http_signing::{HttpSignatureVerificationInput, SignatureEvidenceResult};

#[utoipa::path(
    post,
    path = "/ap/profiles/{profile_nanoid}/inbox",
    description = "Signed Profile inbox. Raw Activity bytes relay only when POST_RELAY_INBOX_ENABLED=true (default false: kept off until the ShuttlePub receiver exists; see host decision D68 for the conditions to enable it). Follow/Block are excluded; Undo/Accept/Reject are excluded only when every reference is a known Follow/Block. Create/Update/Delete/Like/Announce, unknown references and extension types forward unchanged. Existing Follow/domain-control and early-Undo rules remain. Body <=1 MiB; duplicate keys at every depth are rejected. ActivityPub JSON and ld+json with supported parameters are accepted. 202 follows durable commit; Profile overload is 429 and global/physical capacity unavailable is 503 with Retry-After.",
    params(("profile_nanoid" = String, Path, description = "Profile nanoid")),
    request_body(content((serde_json::Value = "application/activity+json"), (serde_json::Value = "application/ld+json"))),
    responses(
        (status = 202, description = "Activity applied, duplicate, or ignored without effect"),
        (status = 400, description = "Malformed ActivityPub activity"),
        (status = 401, description = "Missing or invalid HTTP Signature, or the signing actor is not activity.actor"),
        (status = 404, description = "Profile not found or not an actor"),
        (status = 408, description = "The follower actor could not be resolved (retryable)"),
        (status = 413, description = "Activity body exceeds 1 MiB"),
        (status = 429, description = "Profile admission limit; Retry-After seconds"),
        (status = 503, description = "Global/capacity/storage unavailable; retryable"),
        (status = 422, description = "Activity violates the Profile inbox rules (wrong target Profile, malformed reference)"),
    ),
    tag = "ProfileActor",
)]
pub(crate) async fn post_profile_inbox(
    State(api): State<ActivityPubApi>,
    Path(profile_nanoid): Path<String>,
    OriginalUri(original_uri): OriginalUri,
    method: Method,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ErrorStatus> {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let start = std::time::Instant::now();
    if !media_type_ok(content_type) {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "invalid_content_type".to_string(),
        )));
    }
    ensure_host_matches_public_base_url(&api.public_base_host_header()?, &headers)?;
    let verification_input = HttpSignatureVerificationInput {
        method: method.as_str().to_string(),
        url: format!(
            "{}{}",
            api.public_base_url().as_str().trim_end_matches('/'),
            original_uri
        ),
        headers: headers_to_map(&headers),
        body: Some(body.to_vec()),
    };
    let target = original_uri.to_string();
    if target.len() > REQUEST_TARGET_MAX {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "request_target_too_long".to_string(),
        )));
    }
    let signature_start = std::time::Instant::now();
    let verification = match api.verify_inbox_signature(&verification_input).await {
        Ok(SignatureEvidenceResult::Valid(e)) => e,
        _ => return Err(ErrorStatus::from(StatusCode::UNAUTHORIZED)),
    };
    let signature_us = signature_start.elapsed().as_micros() as u64;
    let read_start = std::time::Instant::now();
    let reading = read_inbox(&body, content_type).map_err(|e| {
        ErrorStatus::from((
            StatusCode::from_u16(e.status).unwrap_or(StatusCode::BAD_REQUEST),
            e.reason.to_string(),
        ))
    })?;
    if !(application::inbox_reading::is_remote_url(&verification.key_id)
        && application::inbox_reading::is_remote_url(&verification.owner)
        && same_activitypub_id(&verification.owner, &reading.actor)
        && signature_key_document_matches_actor(&verification.key_id, &reading.actor))
    {
        return Err(ErrorStatus::from(StatusCode::UNAUTHORIZED));
    }
    let fields = headers
        .iter()
        .map(|(n, v)| {
            (
                n.as_str().to_string(),
                String::from_utf8_lossy(v.as_bytes()).into_owned(),
            )
        })
        .collect::<Vec<_>>();
    let evidence = build_evidence(&fields);
    let read_us = read_start.elapsed().as_micros() as u64;
    let admission_start = std::time::Instant::now();
    api.admit_profile_inbox(ProfileRelayInboxDto {
        profile_nanoid,
        reading,
        raw_body: body.to_vec(),
        verification,
        request_target: target,
        evidence,
        enabled: api.relay_inbox_enabled(),
    })
    .await
    .map_err(|e| {
        if let Some(limit) = e.frames().find_map(|f| f.downcast_ref::<InboxOverload>()) {
            crate::post_relay_refusals::record(limit.reason);
            return ErrorStatus::RetryAfter(
                StatusCode::from_u16(limit.status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
                limit.retry_after_secs,
            );
        }
        // Never log database errors containing binds, malformed body or peer-selected IDs.
        let status = match e.current_context() {
            kernel::KernelError::NotFound => StatusCode::NOT_FOUND,
            kernel::KernelError::Rejected => StatusCode::UNPROCESSABLE_ENTITY,
            kernel::KernelError::Validation => StatusCode::BAD_REQUEST,
            kernel::KernelError::PermissionDenied => StatusCode::UNAUTHORIZED,
            kernel::KernelError::Timeout => StatusCode::REQUEST_TIMEOUT,
            _ => StatusCode::SERVICE_UNAVAILABLE,
        };
        if status == StatusCode::SERVICE_UNAVAILABLE {
            crate::post_relay_refusals::record_storage();
            ErrorStatus::RetryAfter(status, 1)
        } else {
            ErrorStatus::from(status)
        }
    })?;
    tracing::debug!(
        signature_us,
        read_us,
        admission_us = admission_start.elapsed().as_micros() as u64,
        accepted_us = start.elapsed().as_micros() as u64,
        "Profile inbox committed"
    );
    Ok(StatusCode::ACCEPTED.into_response())
}

/// Mounts the Profile inbox on the production root router.
pub trait ProfileInboxRouter {
    /// Routes relative to `/ap`.
    fn route_profile_inbox(self) -> Self;
}

impl ProfileInboxRouter for Router<AppModule> {
    fn route_profile_inbox(self) -> Self {
        self.route(
            "/profiles/{profile_nanoid}/inbox",
            post(post_profile_inbox).layer(DefaultBodyLimit::max(1024 * 1024)),
        )
    }
}

#[cfg(test)]
#[path = "profile_inbox_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "profile_relay_tests.rs"]
mod relay_tests;

#[cfg(test)]
#[path = "profile_domain_guard_tests.rs"]
mod domain_guard_tests;
