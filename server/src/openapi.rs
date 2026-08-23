use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi};

#[allow(dead_code)] // utoipa OpenApiマクロ内部で使用される
struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme(
                "bearer_auth",
                SecurityScheme::Http(
                    HttpBuilder::new()
                        .scheme(HttpAuthScheme::Bearer)
                        .bearer_format("JWT")
                        .build(),
                ),
            );
        }
    }
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Emumet Account Service API",
        version = "0.1.0",
        description = "Account Service for ShuttlePub",
        license(name = "AGPL-3.0", url = "https://www.gnu.org/licenses/agpl-3.0.html")
    ),
    paths(
        crate::route::account::get_accounts,
        crate::route::account::get_account_by_id,
        crate::route::account::create_account,
        crate::route::account::update_account_by_id,
        crate::route::account::deactivate_account_by_id,
        crate::route::account::reactivate_account_by_id,
        crate::route::account::suspend_account_by_id,
        crate::route::account::unsuspend_account_by_id,
        crate::route::account::ban_account_by_id,
        crate::route::account::unban_account_by_id,
        crate::route::account::assign_instance_role,
        crate::route::account::revoke_instance_role,
        crate::route::account::follow_account,
        crate::route::account::unfollow_account,
        crate::route::account::get_followers,
        crate::route::account::get_following,
        crate::route::account::block_account,
        crate::route::account::unblock_account,
        crate::route::account::get_blocks,
        crate::route::account::mute_account,
        crate::route::account::unmute_account,
        crate::route::account::get_mutes,
        crate::route::me::get_me,
        crate::route::media::upload_image,
        crate::route::oauth2::login,
        crate::route::oauth2::get_consent,
        crate::route::oauth2::post_consent,
        crate::route::signing::sign_request,
        crate::route::signing::get_public_key,
        crate::route::activitypub::webfinger,
        crate::route::activitypub::get_actor,
        crate::route::activitypub::post_inbox,
        crate::route::activitypub::get_outbox,
        crate::route::activitypub::get_followers,
        crate::route::activitypub::get_following,
        crate::route::report::create_report,
        crate::route::report::list_reports,
        crate::route::report::resolve_report,
        crate::route::report::dismiss_report,
    ),
    components(schemas(
        crate::schema::account::CreateAccountRequest,
        crate::schema::account::UpdateAccountRequest,
        crate::schema::account::SuspendAccountRequest,
        crate::schema::account::BanAccountRequest,
        crate::schema::account::AccountResponse,
        crate::schema::account::AccountField,
        crate::schema::account::ModerationResponse,
        crate::schema::account::AccountsResponse,
        crate::schema::oauth2::OAuth2Response,
        crate::schema::oauth2::ConsentDecision,
        crate::route::signing::SignRequestBody,
        crate::route::signing::SignResponse,
        crate::route::signing::PublicKeyResponse,
        kernel::activitypub::WebFingerResponse,
        kernel::activitypub::WebFingerLink,
        kernel::activitypub::Actor,
        kernel::activitypub::OrderedCollection,
        kernel::activitypub::PublicKey,
        kernel::activitypub::ImageObject,
        crate::schema::account::FollowAccountRequest,
        crate::schema::account::FollowAccountResponse,
        crate::schema::account::BlockAccountRequest,
        crate::schema::account::MuteAccountRequest,
        crate::schema::account::RelationResponse,
        crate::schema::account::RelationListResponse,
        crate::schema::me::MeResponse,
        crate::schema::media::UploadedImageResponse,
        crate::schema::report::CreateReportRequest,
        crate::schema::report::CloseReportRequest,
        crate::schema::report::AccountReportResponse,
        crate::schema::report::AccountReportListResponse,
    )),
    modifiers(&SecurityAddon),
    tags(
        (name = "Account", description = "Account management"),
        (name = "Me", description = "Authenticated session"),
        (name = "Media", description = "Image uploads"),
        (name = "OAuth2", description = "OAuth2 Login/Consent Provider"),
        (name = "Signing", description = "HTTP Signature signing"),
        (name = "ActivityPub", description = "ActivityPub discovery and actor endpoints"),
        (name = "Report", description = "Account moderation reports"),
    )
)]
#[allow(dead_code)] // utoipa OpenApiマクロ内部で使用される
pub struct ApiDoc;

#[cfg(test)]
pub fn generate_openapi_json() -> String {
    ApiDoc::openapi()
        .to_pretty_json()
        .expect("Failed to serialize OpenAPI spec")
}

#[cfg(test)]
#[path = "openapi/tests.rs"]
mod tests;
