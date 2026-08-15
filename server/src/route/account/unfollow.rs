use crate::auth::{resolve_auth_account_id, AuthClaims, OidcAuthInfo};
use crate::error::ErrorStatus;
use crate::handler::AppModule;
use crate::schema::account::FollowAccountRequest;
use application::dto::activitypub::SendUndoFollowDto;
use application::service::activitypub::SendUndoFollowUseCase;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};

#[utoipa::path(
    post,
    path = "/api/v1/accounts/{account_id}/unfollow",
    description = "Unfollow a remote ActivityPub account by delivering a signed Undo(Follow).",
    params(("account_id" = String, Path, description = "Local account nanoid")),
    request_body = FollowAccountRequest,
    responses(
        (status = 204, description = "Follow removed"),
        (status = 400, description = "Invalid request"),
        (status = 403, description = "Insufficient permission"),
        (status = 404, description = "Account or follow relationship not found"),
        (status = 422, description = "Undo delivery failed"),
    ),
    security(("bearer_auth" = [])),
    tag = "ActivityPub",
)]
pub(crate) async fn unfollow_account(
    Extension(claims): Extension<AuthClaims>,
    State(module): State<AppModule>,
    Path(account_id): Path<String>,
    Json(request): Json<FollowAccountRequest>,
) -> Result<StatusCode, ErrorStatus> {
    if account_id.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "Account ID cannot be empty".to_string(),
        )
            .into());
    }
    if request.target.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "Target cannot be empty".to_string(),
        )
            .into());
    }
    let auth_account_id = resolve_auth_account_id(&module, OidcAuthInfo::from(claims))
        .await
        .map_err(ErrorStatus::from)?;
    module
        .send_undo_follow(
            auth_account_id,
            SendUndoFollowDto {
                account_nanoid: account_id,
                target: request.target,
            },
        )
        .await
        .map_err(ErrorStatus::from)?;
    Ok(StatusCode::NO_CONTENT)
}
