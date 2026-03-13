use crate::auth::{resolve_auth_account_id, AuthClaims, OidcAuthInfo};
use crate::error::ErrorStatus;
use crate::handler::AppModule;
use crate::route::parse_comma_ids;
use crate::schema::metadata::{
    CreateMetadataRequest, GetMetadataQuery, MetadataResponse, UpdateMetadataRequest,
};
use application::service::metadata::{
    CreateMetadataUseCase, DeleteMetadataUseCase, EditMetadataUseCase, GetMetadataUseCase,
};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use axum::{Extension, Json, Router};

pub trait MetadataRouter {
    fn route_metadata(self) -> Self;
}

#[utoipa::path(
    get,
    path = "/metadata",
    description = "Retrieve metadata entries for the given account IDs.",
    params(("account_ids" = String, Query, description = "Comma-separated account IDs")),
    responses(
        (status = 200, description = "List of metadata", body = Vec<MetadataResponse>),
        (status = 400, description = "Invalid request"),
    ),
    security(("bearer_auth" = [])),
    tag = "Metadata",
)]
pub(crate) async fn get_metadata_batch(
    Extension(claims): Extension<AuthClaims>,
    State(module): State<AppModule>,
    Query(query): Query<GetMetadataQuery>,
) -> Result<Json<Vec<MetadataResponse>>, ErrorStatus> {
    let auth_info = OidcAuthInfo::from(claims);

    let account_ids = parse_comma_ids(&query.account_ids)?;

    let auth_account_id = resolve_auth_account_id(&module, auth_info)
        .await
        .map_err(ErrorStatus::from)?;

    let metadata_list = module
        .get_metadata_batch(&auth_account_id, account_ids)
        .await
        .map_err(ErrorStatus::from)?;

    Ok(Json(
        metadata_list
            .into_iter()
            .map(MetadataResponse::from)
            .collect(),
    ))
}

#[utoipa::path(
    post,
    path = "/accounts/{account_id}/metadata",
    description = "Create a metadata entry for the specified account.",
    params(("account_id" = String, Path, description = "Account nanoid")),
    request_body = CreateMetadataRequest,
    responses(
        (status = 201, description = "Metadata created", body = MetadataResponse),
        (status = 400, description = "Invalid request"),
    ),
    security(("bearer_auth" = [])),
    tag = "Metadata",
)]
pub(crate) async fn create_metadata(
    Extension(claims): Extension<AuthClaims>,
    State(module): State<AppModule>,
    Path(account_id): Path<String>,
    Json(body): Json<CreateMetadataRequest>,
) -> Result<(StatusCode, Json<MetadataResponse>), ErrorStatus> {
    let auth_info = OidcAuthInfo::from(claims);

    if account_id.trim().is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Account ID cannot be empty".to_string(),
        )));
    }

    let auth_account_id = resolve_auth_account_id(&module, auth_info)
        .await
        .map_err(ErrorStatus::from)?;

    let metadata = module
        .create_metadata(&auth_account_id, account_id, body.label, body.content)
        .await
        .map_err(ErrorStatus::from)?;

    Ok((StatusCode::CREATED, Json(MetadataResponse::from(metadata))))
}

#[utoipa::path(
    put,
    path = "/accounts/{account_id}/metadata/{id}",
    description = "Update a metadata entry.",
    params(
        ("account_id" = String, Path, description = "Account nanoid"),
        ("id" = String, Path, description = "Metadata nanoid"),
    ),
    request_body = UpdateMetadataRequest,
    responses(
        (status = 204, description = "Metadata updated"),
        (status = 400, description = "Invalid request"),
    ),
    security(("bearer_auth" = [])),
    tag = "Metadata",
)]
pub(crate) async fn update_metadata(
    Extension(claims): Extension<AuthClaims>,
    State(module): State<AppModule>,
    Path((account_id, metadata_id)): Path<(String, String)>,
    Json(body): Json<UpdateMetadataRequest>,
) -> Result<StatusCode, ErrorStatus> {
    let auth_info = OidcAuthInfo::from(claims);

    if account_id.trim().is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Account ID cannot be empty".to_string(),
        )));
    }

    if metadata_id.trim().is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Metadata ID cannot be empty".to_string(),
        )));
    }

    let auth_account_id = resolve_auth_account_id(&module, auth_info)
        .await
        .map_err(ErrorStatus::from)?;

    module
        .edit_metadata(
            &auth_account_id,
            account_id,
            metadata_id,
            body.label,
            body.content,
        )
        .await
        .map_err(ErrorStatus::from)?;

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    delete,
    path = "/accounts/{account_id}/metadata/{id}",
    description = "Delete a metadata entry.",
    params(
        ("account_id" = String, Path, description = "Account nanoid"),
        ("id" = String, Path, description = "Metadata nanoid"),
    ),
    responses(
        (status = 204, description = "Metadata deleted"),
        (status = 400, description = "Invalid request"),
    ),
    security(("bearer_auth" = [])),
    tag = "Metadata",
)]
pub(crate) async fn delete_metadata(
    Extension(claims): Extension<AuthClaims>,
    State(module): State<AppModule>,
    Path((account_id, metadata_id)): Path<(String, String)>,
) -> Result<StatusCode, ErrorStatus> {
    let auth_info = OidcAuthInfo::from(claims);

    if account_id.trim().is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Account ID cannot be empty".to_string(),
        )));
    }

    if metadata_id.trim().is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Metadata ID cannot be empty".to_string(),
        )));
    }

    let auth_account_id = resolve_auth_account_id(&module, auth_info)
        .await
        .map_err(ErrorStatus::from)?;

    module
        .delete_metadata(&auth_account_id, account_id, metadata_id)
        .await
        .map_err(ErrorStatus::from)?;

    Ok(StatusCode::NO_CONTENT)
}

impl MetadataRouter for Router<AppModule> {
    fn route_metadata(self) -> Self {
        self.route("/metadata", get(get_metadata_batch))
            .route("/accounts/:account_id/metadata", post(create_metadata))
            .route(
                "/accounts/:account_id/metadata/:id",
                put(update_metadata).delete(delete_metadata),
            )
    }
}

#[cfg(test)]
mod tests {
    use crate::schema::metadata::MetadataResponse;
    use application::transfer::metadata::MetadataDto;

    #[test]
    fn test_metadata_response_from_dto() {
        let dto = MetadataDto {
            account_nanoid: "acc-123".to_string(),
            nanoid: "test-nanoid".to_string(),
            label: "test-label".to_string(),
            content: "test-content".to_string(),
        };

        let response = MetadataResponse::from(dto);

        assert_eq!(response.account_id, "acc-123");
        assert_eq!(response.nanoid, "test-nanoid");
        assert_eq!(response.label, "test-label");
        assert_eq!(response.content, "test-content");
    }
}
