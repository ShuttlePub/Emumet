use crate::error::ErrorStatus;
use crate::handler::AppModule;
use adapter::processor::account::{AccountQueryProcessor, DependOnAccountQueryProcessor};
use application::service::activitypub::{
    GetActorUseCase, GetFollowersCollectionUseCase, GetOutboxUseCase, GetWebFingerUseCase,
    InboxUseCase,
};
use application::transfer::activitypub::{GetActorDto, GetWebFingerDto, InboxActivityDto};
use axum::body::Bytes;
use axum::extract::DefaultBodyLimit;
use axum::extract::{OriginalUri, Path, Query, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use kernel::interfaces::config::DependOnPublicBaseUrl;
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::http_signing::{
    DependOnHttpSignatureVerifier, HttpSignatureVerificationInput, HttpSignatureVerifier,
    SignatureVerificationResult,
};
use kernel::prelude::entity::{Account, AccountId, Nanoid};
use serde::Serialize;
use std::collections::HashMap;

const ACTIVITY_JSON: &str = "application/activity+json";
const ACTIVITY_LD_JSON: &str = "application/ld+json";
const ACTIVITYSTREAMS_PROFILE: &str = "https://www.w3.org/ns/activitystreams";
const JRD_JSON: &str = "application/jrd+json";

pub trait ActivityPubRouter {
    fn route_activitypub(self) -> Self;
}

impl ActivityPubRouter for Router<AppModule> {
    fn route_activitypub(self) -> Self {
        self.route("/.well-known/webfinger", get(webfinger))
            .route("/accounts/{account_id}", get(get_actor))
            .route(
                "/accounts/{account_id}/inbox",
                post(post_inbox).layer(DefaultBodyLimit::max(1024 * 1024)),
            )
            .route("/accounts/{account_id}/outbox", get(get_outbox))
            .route("/accounts/{account_id}/followers", get(get_followers))
            .route("/accounts/{account_id}/following", get(get_following))
    }
}

#[utoipa::path(
    get,
    path = "/.well-known/webfinger",
    description = "WebFinger account discovery for ActivityPub.",
    params(("resource" = String, Query, description = "Resource URI (acct:user@domain)")),
    responses(
        (status = 200, description = "WebFinger response", body = crate::schema::activitypub::WebFingerResponse, content_type = "application/jrd+json"),
        (status = 400, description = "Invalid resource format"),
        (status = 404, description = "Account not found"),
    ),
    tag = "ActivityPub",
)]
pub(crate) async fn webfinger(
    State(module): State<AppModule>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, ErrorStatus> {
    let resource = params.get("resource").ok_or_else(|| {
        ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Missing resource parameter".to_string(),
        ))
    })?;
    let dto = parse_webfinger_resource(resource)?;
    let expected_domain = public_base_host(module.public_base_url().as_str())?;
    if !dto.domain.eq_ignore_ascii_case(&expected_domain) {
        tracing::debug!(resource, expected_domain, "WebFinger domain mismatch");
        return Err(ErrorStatus::from(StatusCode::NOT_FOUND));
    }
    let response = module.get_webfinger(dto).await.map_err(ErrorStatus::from)?;

    json_response(&response, JRD_JSON)
}

#[utoipa::path(
        get,
        path = "/accounts/{account_id}",
        description = "Retrieve an ActivityPub Actor document for a local account.",
        params(("id" = String, Path, description = "Account nanoid")),
    responses(
        (status = 200, description = "ActivityPub Actor", body = crate::schema::activitypub::ActorResponse, content_type = "application/activity+json"),
        (status = 404, description = "Actor not found or ActivityPub media type not requested"),
    ),
    tag = "ActivityPub",
)]
pub(crate) async fn get_actor(
    State(module): State<AppModule>,
    Path(account_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ErrorStatus> {
    if !accepts_activitypub(&headers) {
        return Err(ErrorStatus::from(StatusCode::NOT_FOUND));
    }
    if account_id.trim().is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Account ID cannot be empty".to_string(),
        )));
    }

    let actor = module
        .get_actor(GetActorDto {
            account_nanoid: account_id.clone(),
        })
        .await
        .map_err(|e| {
            tracing::debug!(nanoid = %account_id, error = ?e, "Actor not found");
            ErrorStatus::from(e)
        })?;

    json_response(&actor, ACTIVITY_JSON)
}

#[utoipa::path(
        get,
        path = "/accounts/{account_id}/followers",
        description = "Retrieve an ActivityPub followers OrderedCollection for a local account.",
        params(("id" = String, Path, description = "Account nanoid")),
    responses(
        (status = 200, description = "Followers collection", body = crate::schema::activitypub::OrderedCollectionResponse, content_type = "application/activity+json"),
        (status = 400, description = "Invalid account ID"),
        (status = 404, description = "Account not found"),
    ),
    tag = "ActivityPub",
)]
pub(crate) async fn get_followers(
    State(module): State<AppModule>,
    Path(account_id): Path<String>,
) -> Result<Response, ErrorStatus> {
    let account_id = find_account_id_by_nanoid(&module, account_id.clone())
        .await
        .map_err(|e| {
            tracing::debug!(nanoid = %account_id, error = ?e, "Followers collection not found");
            e
        })?;
    let collection = module
        .get_followers_collection(&account_id)
        .await
        .map_err(ErrorStatus::from)?;

    json_response(&collection, ACTIVITY_JSON)
}

#[utoipa::path(
        get,
        path = "/accounts/{account_id}/following",
        description = "Retrieve an ActivityPub following OrderedCollection for a local account.",
        params(("id" = String, Path, description = "Account nanoid")),
    responses(
        (status = 200, description = "Following collection", body = crate::schema::activitypub::OrderedCollectionResponse, content_type = "application/activity+json"),
        (status = 400, description = "Invalid account ID"),
        (status = 404, description = "Account not found"),
    ),
    tag = "ActivityPub",
)]
pub(crate) async fn get_following(
    State(module): State<AppModule>,
    Path(account_id): Path<String>,
) -> Result<Response, ErrorStatus> {
    let account_id = find_account_id_by_nanoid(&module, account_id.clone())
        .await
        .map_err(|e| {
            tracing::debug!(nanoid = %account_id, error = ?e, "Following collection not found");
            e
        })?;
    let collection = module
        .get_following_collection(&account_id)
        .await
        .map_err(ErrorStatus::from)?;

    json_response(&collection, ACTIVITY_JSON)
}

#[utoipa::path(
        get,
        path = "/accounts/{account_id}/outbox",
        description = "Retrieve an ActivityPub outbox OrderedCollection for a local account.",
        params(
            ("id" = String, Path, description = "Account nanoid"),
        ("limit" = Option<usize>, Query, description = "Maximum number of activities to return"),
        ("cursor" = Option<i64>, Query, description = "Return activities with IDs older than this cursor")
    ),
    responses(
        (status = 200, description = "Outbox collection", body = crate::schema::activitypub::OrderedCollectionResponse, content_type = "application/activity+json"),
        (status = 400, description = "Invalid account ID or pagination parameter"),
        (status = 404, description = "Account not found"),
    ),
    tag = "ActivityPub",
)]
pub(crate) async fn get_outbox(
    State(module): State<AppModule>,
    Path(account_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, ErrorStatus> {
    let account_id = find_account_id_by_nanoid(&module, account_id.clone())
        .await
        .map_err(|e| {
            tracing::debug!(nanoid = %account_id, error = ?e, "Outbox collection not found");
            e
        })?;
    let limit = parse_limit(params.get("limit"))?;
    let cursor = parse_cursor(params.get("cursor"))?;
    let collection = module
        .get_outbox_collection(&account_id, limit, cursor)
        .await
        .map_err(ErrorStatus::from)?;

    json_response(&collection, ACTIVITY_JSON)
}

#[utoipa::path(
        post,
        path = "/accounts/{account_id}/inbox",
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

async fn find_account_id_by_nanoid(
    module: &AppModule,
    id: String,
) -> Result<AccountId, ErrorStatus> {
    if id.trim().is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Account ID cannot be empty".to_string(),
        )));
    }

    let mut executor = module
        .database_connection()
        .get_executor()
        .await
        .map_err(ErrorStatus::from)?;
    let nanoid = Nanoid::<Account>::new(id);
    module
        .account_query_processor()
        .find_by_nanoid(&mut executor, &nanoid)
        .await
        .map_err(ErrorStatus::from)?
        .map(|account| account.id().clone())
        .ok_or_else(|| ErrorStatus::from(StatusCode::NOT_FOUND))
}

fn json_response<T: Serialize>(
    value: &T,
    content_type: &'static str,
) -> Result<Response, ErrorStatus> {
    let body = serde_json::to_vec(value).map_err(|e| {
        ErrorStatus::from((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to serialize JSON response: {e}"),
        ))
    })?;
    Ok(([(header::CONTENT_TYPE, content_type)], body).into_response())
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
        Ok(())
    } else {
        tracing::warn!(
            expected,
            actual,
            "ActivityPub inbox Host does not match PUBLIC_BASE_URL"
        );
        Err(ErrorStatus::from(StatusCode::UNAUTHORIZED))
    }
}

fn public_base_host_header(base_url: &str) -> Result<String, ErrorStatus> {
    let url = url::Url::parse(base_url).map_err(|e| {
        ErrorStatus::from((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Invalid PUBLIC_BASE_URL: {e}"),
        ))
    })?;
    let host = url.host_str().ok_or_else(|| {
        ErrorStatus::from((
            StatusCode::INTERNAL_SERVER_ERROR,
            "PUBLIC_BASE_URL must include a host".to_string(),
        ))
    })?;
    Ok(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
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

fn parse_webfinger_resource(resource: &str) -> Result<GetWebFingerDto, ErrorStatus> {
    let value = resource
        .strip_prefix("acct:")
        .ok_or_else(invalid_resource)?;
    let (account_name, domain) = value.split_once('@').ok_or_else(invalid_resource)?;
    if account_name.trim().is_empty()
        || domain.trim().is_empty()
        || account_name.contains(char::is_whitespace)
        || domain.contains(char::is_whitespace)
        || domain.contains('@')
    {
        return Err(invalid_resource());
    }
    Ok(GetWebFingerDto {
        account_name: account_name.to_string(),
        domain: domain.to_string(),
    })
}

fn invalid_resource() -> ErrorStatus {
    ErrorStatus::from((
        StatusCode::BAD_REQUEST,
        "Invalid WebFinger resource format".to_string(),
    ))
}

fn parse_limit(limit: Option<&String>) -> Result<usize, ErrorStatus> {
    limit.map_or(Ok(20), |limit| {
        limit.parse::<usize>().map_err(|_| {
            ErrorStatus::from((
                StatusCode::BAD_REQUEST,
                "Invalid limit parameter".to_string(),
            ))
        })
    })
}

fn parse_cursor(cursor: Option<&String>) -> Result<Option<i64>, ErrorStatus> {
    cursor.map_or(Ok(None), |cursor| {
        cursor.parse::<i64>().map(Some).map_err(|_| {
            ErrorStatus::from((
                StatusCode::BAD_REQUEST,
                "Invalid cursor parameter".to_string(),
            ))
        })
    })
}

fn public_base_host(base_url: &str) -> Result<String, ErrorStatus> {
    let url = url::Url::parse(base_url).map_err(|e| {
        ErrorStatus::from((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Invalid PUBLIC_BASE_URL: {e}"),
        ))
    })?;
    url.host_str().map(str::to_string).ok_or_else(|| {
        ErrorStatus::from((
            StatusCode::INTERNAL_SERVER_ERROR,
            "PUBLIC_BASE_URL must include a host".to_string(),
        ))
    })
}

fn accepts_activitypub(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.split(',').any(accept_item_is_activitypub))
}

fn accept_item_is_activitypub(item: &str) -> bool {
    let mut parts = item.split(';').map(str::trim);
    let media_type = parts.next().unwrap_or_default();
    if media_type.eq_ignore_ascii_case(ACTIVITY_JSON) {
        return true;
    }
    if !media_type.eq_ignore_ascii_case(ACTIVITY_LD_JSON) {
        return false;
    }
    parts.any(|part| {
        part.split_once('=').is_some_and(|(key, value)| {
            key.trim().eq_ignore_ascii_case("profile")
                && value.trim().trim_matches('"') == ACTIVITYSTREAMS_PROFILE
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn parse_webfinger_resource_accepts_acct_uri() {
        let dto = parse_webfinger_resource("acct:alice@example.com").unwrap();
        assert_eq!(dto.account_name, "alice");
        assert_eq!(dto.domain, "example.com");
    }

    #[test]
    fn parse_webfinger_resource_rejects_invalid_values() {
        for resource in [
            "alice@example.com",
            "acct:alice",
            "acct:@example.com",
            "acct:alice@",
            "acct:ali ce@example.com",
            "acct:alice@example.com@other.example",
        ] {
            assert!(parse_webfinger_resource(resource).is_err(), "{resource}");
        }
    }

    #[test]
    fn accepts_activitypub_media_types() {
        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT, HeaderValue::from_static(ACTIVITY_JSON));
        assert!(accepts_activitypub(&headers));

        headers.insert(
            header::ACCEPT,
            HeaderValue::from_static(
                "application/ld+json; profile=\"https://www.w3.org/ns/activitystreams\"",
            ),
        );
        assert!(accepts_activitypub(&headers));
    }

    #[test]
    fn rejects_non_activitypub_media_types() {
        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT, HeaderValue::from_static("application/json"));
        assert!(!accepts_activitypub(&headers));
    }

    #[test]
    fn parse_limit_defaults_and_rejects_invalid_values() {
        assert_eq!(parse_limit(None).unwrap(), 20);
        assert_eq!(parse_limit(Some(&"5".to_string())).unwrap(), 5);
        assert!(parse_limit(Some(&"bad".to_string())).is_err());
    }

    #[test]
    fn parse_cursor_accepts_optional_i64() {
        assert_eq!(parse_cursor(None).unwrap(), None);
        assert_eq!(parse_cursor(Some(&"42".to_string())).unwrap(), Some(42));
        assert!(parse_cursor(Some(&"bad".to_string())).is_err());
    }
}
