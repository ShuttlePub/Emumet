use crate::api::AccountApi;
use crate::auth::{AuthClaims, OidcAuthInfo};
use crate::error::ErrorStatus;
use crate::schema::account::{FollowAccountRequest, FollowAccountResponse};
use application::dto::activitypub::SendFollowDto;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};

#[utoipa::path(
    post,
    path = "/api/v1/accounts/{account_id}/follow",
    description = "Follow a remote ActivityPub account.",
    params(("account_id" = String, Path, description = "Local account nanoid")),
    request_body = FollowAccountRequest,
    responses(
        (status = 200, description = "Follow initiated", body = FollowAccountResponse),
        (status = 400, description = "Invalid request"),
        (status = 404, description = "Account not found"),
        (status = 409, description = "Already following"),
    ),
    security(("bearer_auth" = [])),
    tag = "ActivityPub",
)]
pub(crate) async fn follow_account(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AccountApi>,
    Path(account_id): Path<String>,
    Json(request): Json<FollowAccountRequest>,
) -> Result<Json<FollowAccountResponse>, ErrorStatus> {
    let auth_info = OidcAuthInfo::from(claims);

    if account_id.trim().is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Account ID cannot be empty".to_string(),
        )));
    }

    if request.target.trim().is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Target cannot be empty".to_string(),
        )));
    }

    let auth_account_id = api
        .resolve_auth_account_id(auth_info)
        .await
        .map_err(ErrorStatus::from)?;

    let result = api
        .send_follow(
            auth_account_id,
            SendFollowDto {
                account_nanoid: account_id,
                target: request.target,
            },
        )
        .await
        .map_err(ErrorStatus::from)?;

    Ok(Json(FollowAccountResponse {
        follow_id: result.follow_id,
        remote_actor_url: result.remote_actor_url,
        activity_id: result.activity_id,
        approved: result.approved,
    }))
}
