use crate::error::ErrorStatus;
use crate::expect_role;
use crate::handler::AppModule;
use crate::keycloak::{resolve_auth_account_id, KeycloakAuthAccount};
use application::service::metadata::{
    CreateMetadataUseCase, DeleteMetadataUseCase, EditMetadataUseCase, GetMetadataUseCase,
};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::http::{Method, Uri};
use axum::routing::{get, put};
use axum::{Extension, Json, Router};
use axum_keycloak_auth::decode::KeycloakToken;
use axum_keycloak_auth::instance::KeycloakAuthInstance;
use axum_keycloak_auth::layer::KeycloakAuthLayer;
use axum_keycloak_auth::PassthroughMode;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
struct CreateMetadataRequest {
    label: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct UpdateMetadataRequest {
    label: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct MetadataResponse {
    nanoid: String,
    label: String,
    content: String,
}

impl From<application::transfer::metadata::MetadataDto> for MetadataResponse {
    fn from(dto: application::transfer::metadata::MetadataDto) -> Self {
        Self {
            nanoid: dto.nanoid,
            label: dto.label,
            content: dto.content,
        }
    }
}

pub trait MetadataRouter {
    fn route_metadata(self, keycloak: Arc<KeycloakAuthInstance>) -> Self;
}

async fn get_metadata(
    Extension(token): Extension<KeycloakToken<String>>,
    State(module): State<AppModule>,
    method: Method,
    uri: Uri,
    Path(account_id): Path<String>,
) -> Result<Json<Vec<MetadataResponse>>, ErrorStatus> {
    expect_role!(&token, uri, method);
    let auth_info = KeycloakAuthAccount::from(token);

    if account_id.trim().is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Account ID cannot be empty".to_string(),
        )));
    }

    let auth_account_id = resolve_auth_account_id(&module, auth_info)
        .await
        .map_err(ErrorStatus::from)?;

    let metadata_list = module
        .get_metadata(&auth_account_id, account_id)
        .await
        .map_err(ErrorStatus::from)?;

    Ok(Json(
        metadata_list
            .into_iter()
            .map(MetadataResponse::from)
            .collect(),
    ))
}

async fn create_metadata(
    Extension(token): Extension<KeycloakToken<String>>,
    State(module): State<AppModule>,
    method: Method,
    uri: Uri,
    Path(account_id): Path<String>,
    Json(body): Json<CreateMetadataRequest>,
) -> Result<(StatusCode, Json<MetadataResponse>), ErrorStatus> {
    expect_role!(&token, uri, method);
    let auth_info = KeycloakAuthAccount::from(token);

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

async fn update_metadata(
    Extension(token): Extension<KeycloakToken<String>>,
    State(module): State<AppModule>,
    method: Method,
    uri: Uri,
    Path((account_id, metadata_id)): Path<(String, String)>,
    Json(body): Json<UpdateMetadataRequest>,
) -> Result<StatusCode, ErrorStatus> {
    expect_role!(&token, uri, method);
    let auth_info = KeycloakAuthAccount::from(token);

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

async fn delete_metadata(
    Extension(token): Extension<KeycloakToken<String>>,
    State(module): State<AppModule>,
    method: Method,
    uri: Uri,
    Path((account_id, metadata_id)): Path<(String, String)>,
) -> Result<StatusCode, ErrorStatus> {
    expect_role!(&token, uri, method);
    let auth_info = KeycloakAuthAccount::from(token);

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
    fn route_metadata(self, keycloak: Arc<KeycloakAuthInstance>) -> Self {
        self.route(
            "/accounts/:account_id/metadata",
            get(get_metadata).post(create_metadata),
        )
        .route(
            "/accounts/:account_id/metadata/:id",
            put(update_metadata).delete(delete_metadata),
        )
        .layer(
            KeycloakAuthLayer::<String>::builder()
                .instance(keycloak)
                .passthrough_mode(PassthroughMode::Block)
                .expected_audiences(vec![String::from("account")])
                .build(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::transfer::metadata::MetadataDto;

    #[test]
    fn test_metadata_response_from_dto() {
        let dto = MetadataDto {
            nanoid: "test-nanoid".to_string(),
            label: "test-label".to_string(),
            content: "test-content".to_string(),
        };

        let response = MetadataResponse::from(dto);

        assert_eq!(response.nanoid, "test-nanoid");
        assert_eq!(response.label, "test-label");
        assert_eq!(response.content, "test-content");
    }
}
