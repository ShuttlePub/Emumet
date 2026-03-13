use crate::auth::{resolve_auth_account_id, AuthClaims, OidcAuthInfo};
use crate::error::ErrorStatus;
use crate::handler::AppModule;
use crate::route::parse_comma_ids;
use crate::schema::profile::{
    CreateProfileRequest, GetProfilesQuery, ProfileResponse, UpdateProfileRequest,
};
use application::service::profile::{CreateProfileUseCase, EditProfileUseCase, GetProfileUseCase};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use kernel::prelude::entity::ImageId;

pub trait ProfileRouter {
    fn route_profile(self) -> Self;
}

async fn get_profiles_batch(
    Extension(claims): Extension<AuthClaims>,
    State(module): State<AppModule>,
    Query(query): Query<GetProfilesQuery>,
) -> Result<Json<Vec<ProfileResponse>>, ErrorStatus> {
    let auth_info = OidcAuthInfo::from(claims);

    let account_ids = parse_comma_ids(&query.account_ids)?;

    let auth_account_id = resolve_auth_account_id(&module, auth_info)
        .await
        .map_err(ErrorStatus::from)?;

    let profiles = module
        .get_profiles_batch(&auth_account_id, account_ids)
        .await
        .map_err(ErrorStatus::from)?;

    Ok(Json(
        profiles.into_iter().map(ProfileResponse::from).collect(),
    ))
}

async fn create_profile(
    Extension(claims): Extension<AuthClaims>,
    State(module): State<AppModule>,
    Path(account_id): Path<String>,
    Json(body): Json<CreateProfileRequest>,
) -> Result<(StatusCode, Json<ProfileResponse>), ErrorStatus> {
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
    Extension(claims): Extension<AuthClaims>,
    State(module): State<AppModule>,
    Path(account_id): Path<String>,
    Json(body): Json<UpdateProfileRequest>,
) -> Result<StatusCode, ErrorStatus> {
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

impl ProfileRouter for Router<AppModule> {
    fn route_profile(self) -> Self {
        self.route("/profiles", get(get_profiles_batch)).route(
            "/accounts/:account_id/profile",
            post(create_profile).put(update_profile),
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::schema::profile::ProfileResponse;
    use application::transfer::profile::ProfileDto;
    use uuid::Uuid;

    #[test]
    fn test_profile_response_from_dto_with_all_fields() {
        let dto = ProfileDto {
            account_nanoid: "acc-123".to_string(),
            nanoid: "test-nanoid".to_string(),
            display_name: Some("Test User".to_string()),
            summary: Some("A test summary".to_string()),
            icon_id: Some(Uuid::nil()),
            banner_id: Some(Uuid::nil()),
        };

        let response = ProfileResponse::from(dto);

        assert_eq!(response.account_id, "acc-123");
        assert_eq!(response.nanoid, "test-nanoid");
        assert_eq!(response.display_name, Some("Test User".to_string()));
        assert_eq!(response.summary, Some("A test summary".to_string()));
        assert_eq!(response.icon_id, Some(Uuid::nil()));
        assert_eq!(response.banner_id, Some(Uuid::nil()));
    }

    #[test]
    fn test_profile_response_from_dto_with_no_optional_fields() {
        let dto = ProfileDto {
            account_nanoid: "acc-456".to_string(),
            nanoid: "test-nanoid-2".to_string(),
            display_name: None,
            summary: None,
            icon_id: None,
            banner_id: None,
        };

        let response = ProfileResponse::from(dto);

        assert_eq!(response.account_id, "acc-456");
        assert_eq!(response.nanoid, "test-nanoid-2");
        assert!(response.display_name.is_none());
        assert!(response.summary.is_none());
        assert!(response.icon_id.is_none());
        assert!(response.banner_id.is_none());
    }
}
