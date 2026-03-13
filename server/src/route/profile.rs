use crate::auth::{resolve_auth_account_id, AuthClaims, OidcAuthInfo};
use crate::error::ErrorStatus;
use crate::handler::AppModule;
use crate::route::parse_comma_ids;
use crate::schema::profile::{
    into_field_action, CreateProfileRequest, GetProfilesQuery, ProfileResponse,
    UpdateProfileRequest,
};
use application::service::profile::{CreateProfileUseCase, EditProfileUseCase, GetProfileUseCase};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};

pub trait ProfileRouter {
    fn route_profile(self) -> Self;
}

#[utoipa::path(
    get,
    path = "/profiles",
    description = "Retrieve profiles for the given account IDs.",
    params(("account_ids" = String, Query, description = "Comma-separated account IDs")),
    responses(
        (status = 200, description = "List of profiles", body = Vec<ProfileResponse>),
        (status = 400, description = "Invalid request"),
    ),
    security(("bearer_auth" = [])),
    tag = "Profile",
)]
pub(crate) async fn get_profiles_batch(
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

#[utoipa::path(
    post,
    path = "/accounts/{account_id}/profile",
    description = "Create a profile for the specified account.",
    params(("account_id" = String, Path, description = "Account nanoid")),
    request_body = CreateProfileRequest,
    responses(
        (status = 201, description = "Profile created", body = ProfileResponse),
        (status = 400, description = "Invalid request"),
    ),
    security(("bearer_auth" = [])),
    tag = "Profile",
)]
pub(crate) async fn create_profile(
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

    let profile = module
        .create_profile(
            &auth_account_id,
            account_id,
            body.display_name,
            body.summary,
            body.icon_url,
            body.banner_url,
        )
        .await
        .map_err(ErrorStatus::from)?;

    Ok((StatusCode::CREATED, Json(ProfileResponse::from(profile))))
}

#[utoipa::path(
    put,
    path = "/accounts/{account_id}/profile",
    description = "Update the profile of the specified account.",
    params(("account_id" = String, Path, description = "Account nanoid")),
    request_body = UpdateProfileRequest,
    responses(
        (status = 204, description = "Profile updated"),
        (status = 400, description = "Invalid request"),
    ),
    security(("bearer_auth" = [])),
    tag = "Profile",
)]
pub(crate) async fn update_profile(
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

    module
        .edit_profile(
            &auth_account_id,
            account_id,
            body.display_name,
            body.summary,
            into_field_action(body.icon_url),
            into_field_action(body.banner_url),
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
    use crate::schema::profile::{ProfileResponse, UpdateProfileRequest};
    use application::transfer::profile::ProfileDto;

    #[test]
    fn test_profile_response_from_dto_with_all_fields() {
        let dto = ProfileDto {
            account_nanoid: "acc-123".to_string(),
            nanoid: "test-nanoid".to_string(),
            display_name: Some("Test User".to_string()),
            summary: Some("A test summary".to_string()),
            icon_url: Some("https://example.com/icon.png".to_string()),
            banner_url: Some("https://example.com/banner.png".to_string()),
        };

        let response = ProfileResponse::from(dto);

        assert_eq!(response.account_id, "acc-123");
        assert_eq!(response.nanoid, "test-nanoid");
        assert_eq!(response.display_name, Some("Test User".to_string()));
        assert_eq!(response.summary, Some("A test summary".to_string()));
        assert_eq!(
            response.icon_url,
            Some("https://example.com/icon.png".to_string())
        );
        assert_eq!(
            response.banner_url,
            Some("https://example.com/banner.png".to_string())
        );
    }

    #[test]
    fn test_profile_response_from_dto_with_no_optional_fields() {
        let dto = ProfileDto {
            account_nanoid: "acc-456".to_string(),
            nanoid: "test-nanoid-2".to_string(),
            display_name: None,
            summary: None,
            icon_url: None,
            banner_url: None,
        };

        let response = ProfileResponse::from(dto);

        assert_eq!(response.account_id, "acc-456");
        assert_eq!(response.nanoid, "test-nanoid-2");
        assert!(response.display_name.is_none());
        assert!(response.summary.is_none());
        assert!(response.icon_url.is_none());
        assert!(response.banner_url.is_none());
    }

    #[test]
    fn test_update_request_absent_fields() {
        let json = r#"{"display_name": "test"}"#;
        let req: UpdateProfileRequest = serde_json::from_str(json).unwrap();
        assert!(req.icon_url.is_none());
        assert!(req.banner_url.is_none());
    }

    #[test]
    fn test_update_request_null_fields() {
        let json = r#"{"icon_url": null, "banner_url": null}"#;
        let req: UpdateProfileRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.icon_url, Some(None));
        assert_eq!(req.banner_url, Some(None));
    }

    #[test]
    fn test_update_request_set_fields() {
        let json = r#"{"icon_url": "https://example.com/icon.png", "banner_url": "https://example.com/banner.png"}"#;
        let req: UpdateProfileRequest = serde_json::from_str(json).unwrap();
        assert_eq!(
            req.icon_url,
            Some(Some("https://example.com/icon.png".to_string()))
        );
        assert_eq!(
            req.banner_url,
            Some(Some("https://example.com/banner.png".to_string()))
        );
    }
}
