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
        crate::route::account::create_account,
        crate::route::account::update_account_by_id,
        crate::route::account::deactivate_account_by_id,
        crate::route::account::suspend_account_by_id,
        crate::route::account::unsuspend_account_by_id,
        crate::route::account::ban_account_by_id,
        crate::route::profile::get_profiles_batch,
        crate::route::profile::create_profile,
        crate::route::profile::update_profile,
        crate::route::metadata::get_metadata_batch,
        crate::route::metadata::create_metadata,
        crate::route::metadata::update_metadata,
        crate::route::metadata::delete_metadata,
        crate::route::oauth2::login,
        crate::route::oauth2::get_consent,
        crate::route::oauth2::post_consent,
        crate::route::signing::sign_request,
        crate::route::signing::get_public_key,
    ),
    components(schemas(
        crate::schema::account::CreateAccountRequest,
        crate::schema::account::UpdateAccountRequest,
        crate::schema::account::SuspendAccountRequest,
        crate::schema::account::BanAccountRequest,
        crate::schema::account::AccountResponse,
        crate::schema::account::ModerationResponse,
        crate::schema::account::AccountsResponse,
        crate::schema::profile::CreateProfileRequest,
        crate::schema::profile::UpdateProfileRequest,
        crate::schema::profile::ProfileResponse,
        crate::schema::metadata::CreateMetadataRequest,
        crate::schema::metadata::UpdateMetadataRequest,
        crate::schema::metadata::MetadataResponse,
        crate::schema::oauth2::OAuth2Response,
        crate::schema::oauth2::ConsentDecision,
        crate::route::signing::SignRequestBody,
        crate::route::signing::SignResponse,
        crate::route::signing::PublicKeyResponse,
    )),
    modifiers(&SecurityAddon),
    tags(
        (name = "Account", description = "Account management"),
        (name = "Profile", description = "Profile management"),
        (name = "Metadata", description = "Metadata management"),
        (name = "OAuth2", description = "OAuth2 Login/Consent Provider"),
        (name = "Signing", description = "HTTP Signature signing"),
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
mod tests {
    use super::*;

    #[test]
    #[ignore]
    fn write_openapi_spec_to_file() {
        let json = generate_openapi_json();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("openapi.json");
        std::fs::write(&path, &json).expect("Failed to write openapi.json");
        println!("OpenAPI spec written to {}", path.display());
    }

    #[test]
    fn openapi_spec_matches_committed_file() {
        let generated = generate_openapi_json();

        let parsed: serde_json::Value =
            serde_json::from_str(&generated).expect("Generated spec is not valid JSON");
        assert_eq!(parsed["info"]["title"], "Emumet Account Service API");

        let committed_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("openapi.json");
        let committed = std::fs::read_to_string(&committed_path).unwrap_or_else(|_| {
            panic!(
                "openapi.json not found at {}. Generate with: cargo test -p server write_openapi_spec_to_file -- --ignored",
                committed_path.display()
            )
        });
        assert_eq!(
            committed, generated,
            "openapi.json is out of date. Regenerate with: cargo test -p server write_openapi_spec_to_file -- --ignored"
        );
    }
}
