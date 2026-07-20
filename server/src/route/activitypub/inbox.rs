use super::{find_account_id_by_nanoid, public_base_host_header};
use crate::error::ErrorStatus;
use crate::handler::AppModule;
use application::service::activitypub::InboxUseCase;
use application::transfer::activitypub::InboxActivityDto;
use axum::body::Bytes;
use axum::extract::{OriginalUri, Path, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use kernel::interfaces::config::DependOnPublicBaseUrl;
use kernel::interfaces::http_signing::{
    DependOnHttpSignatureVerifier, HttpSignatureVerificationInput, HttpSignatureVerifier,
    SignatureVerificationResult,
};
use std::collections::HashMap;

#[utoipa::path(
        post,
        path = "/ap/accounts/{account_id}/inbox",
        description = "ActivityPub inbox for signed inbound federation activities.",
        params(("id" = String, Path, description = "Account nanoid")),
    request_body(content = serde_json::Value, content_type = "application/activity+json"),
    responses(
        (status = 202, description = "Activity accepted or ignored"),
        (status = 400, description = "Malformed ActivityPub activity"),
        (status = 401, description = "Missing or invalid HTTP Signature"),
        (status = 404, description = "Local actor not found"),
    ),
    tag = "ActivityPub",
)]
pub(crate) async fn post_inbox(
    State(module): State<AppModule>,
    Path(nanoid): Path<String>,
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
    let account_id = find_account_id_by_nanoid(&module, nanoid.clone()).await?;
    let verification_input = HttpSignatureVerificationInput {
        method: method.as_str().to_string(),
        url: format!(
            "{}{}",
            module.public_base_url().as_str().trim_end_matches('/'),
            original_uri
        ),
        headers: headers_to_map(&headers),
        body: Some(body.to_vec()),
    };
    ensure_host_matches_public_base_url(&module, &headers)?;
    let key_id = match module
        .http_signature_verifier()
        .verify(&verification_input)
        .await
        .map_err(ErrorStatus::from)?
    {
        SignatureVerificationResult::Valid { key_id } => key_id,
        SignatureVerificationResult::Invalid(reason) => {
            tracing::warn!(
                reason,
                "Rejected ActivityPub inbox request with invalid signature"
            );
            return Err(ErrorStatus::from(StatusCode::UNAUTHORIZED));
        }
    };

    let activity = serde_json::from_slice(&body).map_err(|e| {
        ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            format!("Malformed ActivityPub activity: {e}"),
        ))
    })?;
    ensure_signature_owner_matches_actor(&module, &key_id, &activity).await?;
    module
        .handle_inbox_activity(InboxActivityDto {
            account_id,
            account_nanoid: nanoid,
            activity,
        })
        .await
        .map_err(ErrorStatus::from)?;

    Ok(StatusCode::ACCEPTED.into_response())
}

fn headers_to_map(headers: &HeaderMap) -> HashMap<String, String> {
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

async fn ensure_signature_owner_matches_actor(
    module: &AppModule,
    key_id: &str,
    activity: &kernel::activitypub::Activity,
) -> Result<(), ErrorStatus> {
    let actor_key = module
        .http_signature_verifier()
        .fetch_actor_key(key_id)
        .await
        .map_err(|e| {
            tracing::warn!(?e, key_id, "Failed to fetch ActivityPub signer actor key");
            ErrorStatus::from(StatusCode::UNAUTHORIZED)
        })?;
    if same_activitypub_id(&actor_key.owner, &activity.actor)
        && signature_key_document_matches_actor(key_id, &activity.actor)
    {
        Ok(())
    } else {
        tracing::warn!(
            key_owner = actor_key.owner,
            key_id,
            activity_actor = activity.actor,
            "ActivityPub signature owner does not match activity actor"
        );
        Err(ErrorStatus::from(StatusCode::UNAUTHORIZED))
    }
}

fn ensure_host_matches_public_base_url(
    module: &AppModule,
    headers: &HeaderMap,
) -> Result<(), ErrorStatus> {
    let expected = public_base_host_header(module.public_base_url().as_str())?;
    let actual = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| ErrorStatus::from(StatusCode::UNAUTHORIZED))?;
    if actual.eq_ignore_ascii_case(&expected) {
        return Ok(());
    }
    // In test mode, also accept localhost connections (e.g., when the test
    // environment routes ActivityPub traffic directly to localhost:8080
    // instead of through the nginx reverse proxy). HTTP Signature verification
    // remains the primary security guard for inbox requests.
    if cfg!(any(test, feature = "test-mode")) {
        if actual.eq_ignore_ascii_case("localhost")
            || actual.to_ascii_lowercase().starts_with("localhost:")
        {
            tracing::debug!(
                expected,
                actual,
                "ActivityPub inbox Host is localhost (test-mode override)"
            );
            return Ok(());
        }
    }
    tracing::warn!(
        expected,
        actual,
        "ActivityPub inbox Host does not match PUBLIC_BASE_URL"
    );
    Err(ErrorStatus::from(StatusCode::UNAUTHORIZED))
}

fn same_activitypub_id(left: &str, right: &str) -> bool {
    left.trim_end_matches('/') == right.trim_end_matches('/')
}

fn signature_key_document_matches_actor(key_id: &str, actor: &str) -> bool {
    let Ok(mut key_url) = url::Url::parse(key_id) else {
        return false;
    };
    key_url.set_fragment(None);
    same_activitypub_id(key_url.as_str(), actor)
}
