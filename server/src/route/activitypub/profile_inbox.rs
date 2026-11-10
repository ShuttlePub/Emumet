//! Profile ActivityPub inbox (P3i).
//!
//! `POST /ap/profiles/{profile_nanoid}/inbox` receives signed Follow / Accept / Reject /
//! Undo(Follow) activities for one Profile. Like the Profile actor handlers it is NOT part of the
//! production root router (the mount, together with retiring `/ap/accounts/{id}/inbox`, is owned
//! by P4); only tests mount it through [`ProfileInboxRouter`].
//!
//! The route verifies the HTTP Signature and that the key owner is the `activity.actor`; the
//! relation rules (target Profile, original Follow ID, stale / foreign / duplicate handling) live
//! in `application::service::activitypub::ProfileInboxUseCase`.

use super::inbox::{
    ensure_host_matches_public_base_url, headers_to_map, same_activitypub_id,
    signature_key_document_matches_actor,
};
use crate::api::ActivityPubApi;
use crate::error::ErrorStatus;
use crate::handler::AppModule;
use application::dto::profile_inbound::ProfileInboxDto;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, OriginalUri, Path, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use kernel::interfaces::http_signing::{
    HttpSignatureVerificationInput, SignatureVerificationResult,
};

#[utoipa::path(
    post,
    path = "/ap/profiles/{profile_nanoid}/inbox",
    description = "ActivityPub inbox of one Profile actor. NOT MOUNTED on the production router (the production mount is owned by the P4 cutover). \
Requires a valid HTTP Signature whose key owner is `activity.actor`. \
Received activity table: Follow (object = this Profile; stored as an accepted generation and an Accept delivery job is reserved in the same transaction), \
Accept / Reject (object = a Follow this Profile sent, matched by the original Follow activity ID and by both parties), \
Undo (object = a Follow of the signer; an Undo that overtakes its Follow is remembered so the late Follow creates nothing). \
Every other activity (post activities such as Create / Like / EmojiReact / Announce, Block, Undo of a non-Follow, ...) is acknowledged with 202 and changes nothing: post authorization belongs to ShuttlePub and domain policy to D8G. \
Stale (ended generation), foreign (other Profile / other actor), unknown-Follow and duplicate activities are acknowledged with 202 without changing any relation.",
    params(("profile_nanoid" = String, Path, description = "Profile nanoid")),
    request_body(content = serde_json::Value, content_type = "application/activity+json"),
    responses(
        (status = 202, description = "Activity applied, duplicate, or ignored without effect"),
        (status = 400, description = "Malformed ActivityPub activity"),
        (status = 401, description = "Missing or invalid HTTP Signature, or the signing actor is not activity.actor"),
        (status = 404, description = "Profile not found or not an actor"),
        (status = 408, description = "The follower actor could not be resolved (retryable)"),
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
    if content_type != "application/activity+json" {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Content-Type must be application/activity+json".to_string(),
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
    let key_id = match api
        .verify_http_signature(&verification_input)
        .await
        .map_err(ErrorStatus::from)?
    {
        SignatureVerificationResult::Valid { key_id } => key_id,
        SignatureVerificationResult::Invalid(reason) => {
            tracing::warn!(
                reason,
                "Rejected Profile inbox request with invalid signature"
            );
            return Err(ErrorStatus::from(StatusCode::UNAUTHORIZED));
        }
    };

    let activity: kernel::activitypub::Activity = serde_json::from_slice(&body).map_err(|e| {
        ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            format!("Malformed ActivityPub activity: {e}"),
        ))
    })?;
    let actor_key = api.fetch_actor_key(&key_id).await.map_err(|e| {
        tracing::warn!(?e, key_id, "Failed to fetch Profile inbox signer actor key");
        ErrorStatus::from(StatusCode::UNAUTHORIZED)
    })?;
    if !(same_activitypub_id(&actor_key.owner, &activity.actor)
        && signature_key_document_matches_actor(&key_id, &activity.actor))
    {
        tracing::warn!(
            key_owner = actor_key.owner,
            key_id,
            activity_actor = activity.actor,
            "Profile inbox signature owner does not match activity actor"
        );
        return Err(ErrorStatus::from(StatusCode::UNAUTHORIZED));
    }

    let outcome = api
        .handle_profile_inbox_activity(ProfileInboxDto {
            profile_nanoid,
            signer_actor_url: actor_key.owner,
            activity,
        })
        .await
        .map_err(ErrorStatus::from)?;
    tracing::debug!(?outcome, "Processed Profile inbox activity");
    Ok(StatusCode::ACCEPTED.into_response())
}

/// Mounts the Profile inbox. Production code must NOT call this before P4.
#[cfg_attr(not(test), allow(dead_code))]
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
