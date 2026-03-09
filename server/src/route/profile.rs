use crate::error::ErrorStatus;
use crate::expect_role;
use crate::handler::AppModule;
use crate::keycloak::{resolve_auth_account_id, KeycloakAuthAccount};
use application::service::profile::{
    CreateProfileUseCase, DeleteProfileUseCase, EditProfileUseCase, GetProfileUseCase,
};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::http::{Method, Uri};
use axum::routing::{delete, get, post, put};
use axum::{Extension, Json, Router};
use axum_keycloak_auth::decode::KeycloakToken;
use axum_keycloak_auth::instance::KeycloakAuthInstance;
use axum_keycloak_auth::layer::KeycloakAuthLayer;
use axum_keycloak_auth::PassthroughMode;
use kernel::prelude::entity::ImageId;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
struct CreateProfileRequest {
    display_name: Option<String>,
    summary: Option<String>,
    icon: Option<Uuid>,
    banner: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
struct UpdateProfileRequest {
    display_name: Option<String>,
    summary: Option<String>,
    icon: Option<Uuid>,
    banner: Option<Uuid>,
}

#[derive(Debug, Serialize)]
struct ProfileResponse {
    nanoid: String,
    display_name: Option<String>,
    summary: Option<String>,
    icon_id: Option<Uuid>,
    banner_id: Option<Uuid>,
}

impl From<application::transfer::profile::ProfileDto> for ProfileResponse {
    fn from(dto: application::transfer::profile::ProfileDto) -> Self {
        Self {
            nanoid: dto.nanoid,
            display_name: dto.display_name,
            summary: dto.summary,
            icon_id: dto.icon_id,
            banner_id: dto.banner_id,
        }
    }
}

pub trait ProfileRouter {
    fn route_profile(self, keycloak: Arc<KeycloakAuthInstance>) -> Self;
}

async fn get_profile(
    Extension(token): Extension<KeycloakToken<String>>,
    State(module): State<AppModule>,
    method: Method,
    uri: Uri,
    Path(account_id): Path<String>,
) -> Result<Json<ProfileResponse>, ErrorStatus> {
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

    let profile = module
        .get_profile(&auth_account_id, account_id)
        .await
        .map_err(ErrorStatus::from)?;

    Ok(Json(ProfileResponse::from(profile)))
}

async fn create_profile(
    Extension(token): Extension<KeycloakToken<String>>,
    State(module): State<AppModule>,
    method: Method,
    uri: Uri,
    Path(account_id): Path<String>,
    Json(body): Json<CreateProfileRequest>,
) -> Result<(StatusCode, Json<ProfileResponse>), ErrorStatus> {
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

    let icon = body.icon.map(ImageId::new);
    let banner = body.banner.map(ImageId::new);

    let profile = module
        .create_profile(
            &auth_account_id,
            account_id,
            body.display_name,
            body.summary,
            icon,
            banner,
        )
        .await
        .map_err(ErrorStatus::from)?;

    Ok((StatusCode::CREATED, Json(ProfileResponse::from(profile))))
}

async fn update_profile(
    Extension(token): Extension<KeycloakToken<String>>,
    State(module): State<AppModule>,
    method: Method,
    uri: Uri,
    Path(account_id): Path<String>,
    Json(body): Json<UpdateProfileRequest>,
) -> Result<StatusCode, ErrorStatus> {
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

    let icon = body.icon.map(ImageId::new);
    let banner = body.banner.map(ImageId::new);

    module
        .edit_profile(
            &auth_account_id,
            account_id,
            body.display_name,
            body.summary,
            icon,
            banner,
        )
        .await
        .map_err(ErrorStatus::from)?;

    Ok(StatusCode::NO_CONTENT)
}

async fn delete_profile(
    Extension(token): Extension<KeycloakToken<String>>,
    State(module): State<AppModule>,
    method: Method,
    uri: Uri,
    Path(account_id): Path<String>,
) -> Result<StatusCode, ErrorStatus> {
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

    module
        .delete_profile(&auth_account_id, account_id)
        .await
        .map_err(ErrorStatus::from)?;

    Ok(StatusCode::NO_CONTENT)
}

impl ProfileRouter for Router<AppModule> {
    fn route_profile(self, keycloak: Arc<KeycloakAuthInstance>) -> Self {
        self.route(
            "/accounts/:account_id/profile",
            get(get_profile)
                .post(create_profile)
                .put(update_profile)
                .delete(delete_profile),
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
    use application::transfer::profile::ProfileDto;

    #[test]
    fn test_profile_response_from_dto_with_all_fields() {
        let dto = ProfileDto {
            nanoid: "test-nanoid".to_string(),
            display_name: Some("Test User".to_string()),
            summary: Some("A test summary".to_string()),
            icon_id: Some(Uuid::nil()),
            banner_id: Some(Uuid::nil()),
        };

        let response = ProfileResponse::from(dto);

        assert_eq!(response.nanoid, "test-nanoid");
        assert_eq!(response.display_name, Some("Test User".to_string()));
        assert_eq!(response.summary, Some("A test summary".to_string()));
        assert_eq!(response.icon_id, Some(Uuid::nil()));
        assert_eq!(response.banner_id, Some(Uuid::nil()));
    }

    #[test]
    fn test_profile_response_from_dto_with_no_optional_fields() {
        let dto = ProfileDto {
            nanoid: "test-nanoid-2".to_string(),
            display_name: None,
            summary: None,
            icon_id: None,
            banner_id: None,
        };

        let response = ProfileResponse::from(dto);

        assert_eq!(response.nanoid, "test-nanoid-2");
        assert!(response.display_name.is_none());
        assert!(response.summary.is_none());
        assert!(response.icon_id.is_none());
        assert!(response.banner_id.is_none());
    }
}
