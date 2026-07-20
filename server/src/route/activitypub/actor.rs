use super::{json_response, ACTIVITY_JSON};
use crate::error::ErrorStatus;
use crate::handler::AppModule;
use application::service::activitypub::GetActorUseCase;
use application::transfer::activitypub::GetActorDto;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;

#[utoipa::path(
        get,
        path = "/ap/accounts/{account_id}",
        description = "Retrieve an ActivityPub Actor document for a local account.",
        params(("id" = String, Path, description = "Account nanoid")),
    responses(
        (status = 200, description = "ActivityPub Actor", body = kernel::activitypub::Actor, content_type = "application/activity+json"),
        (status = 404, description = "Actor not found"),
    ),
    tag = "ActivityPub",
)]
pub(crate) async fn get_actor(
    State(module): State<AppModule>,
    Path(account_id): Path<String>,
) -> Result<Response, ErrorStatus> {
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
