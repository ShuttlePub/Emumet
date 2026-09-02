use crate::api::MeApi;
use crate::auth::{AuthClaims, OidcAuthInfo, RequestOrganizationContext};
use crate::error::ErrorStatus;
use crate::handler::AppModule;
use crate::schema::me::{MeResponse, OrganizationContextResponse, SessionContextResponse};
use axum::extract::State;
use axum::http::StatusCode;
use axum::{Extension, Json};
use kernel::interfaces::permission::InstanceRole;

pub trait MeRouter {
    fn route_me(self) -> Self;
}

impl MeRouter for axum::Router<AppModule> {
    fn route_me(self) -> Self {
        self.route("/me", axum::routing::get(get_me)).route(
            "/me/session-context",
            axum::routing::get(get_session_context),
        )
    }
}

#[utoipa::path(
    get,
    path = "/api/v1/me",
    description = "Retrieve the authenticated account and direct instance roles.",
    responses(
        (status = 200, description = "Authenticated session", body = MeResponse),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 503, description = "Instance role service unavailable"),
    ),
    security(("bearer_auth" = [])),
    tag = "Me",
)]
pub(crate) async fn get_me(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<MeApi>,
) -> Result<Json<MeResponse>, ErrorStatus> {
    let auth_account_id = api
        .resolve_auth_account_id(OidcAuthInfo::from(claims))
        .await
        .map_err(ErrorStatus::from)?;
    let session_context = api
        .get_session_context(&auth_account_id, None)
        .await
        .map_err(|_| ErrorStatus::StatusCode(StatusCode::SERVICE_UNAVAILABLE))?;
    let instance_roles = session_context
        .instance_roles
        .into_iter()
        .map(|role| match role {
            InstanceRole::Admin => "admin".to_string(),
            InstanceRole::Moderator => "moderator".to_string(),
        })
        .collect();

    Ok(Json(MeResponse {
        account_id: AsRef::<i64>::as_ref(&auth_account_id).to_string(),
        instance_roles,
    }))
}

#[utoipa::path(
    get,
    path = "/api/v1/me/session-context",
    params(("X-Organization-Id" = Option<String>, Header, description = "Organization account nanoid used for this request")),
    responses(
        (status = 200, body = SessionContextResponse),
        (status = 403, description = "Authenticated person is not an active organization member"),
        (status = 404, description = "Organization not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Me",
)]
pub(crate) async fn get_session_context(
    Extension(claims): Extension<AuthClaims>,
    Extension(RequestOrganizationContext(org_context)): Extension<RequestOrganizationContext>,
    State(api): State<MeApi>,
) -> Result<Json<SessionContextResponse>, ErrorStatus> {
    let auth_account_id = api
        .resolve_auth_account_id(OidcAuthInfo::from(claims))
        .await
        .map_err(ErrorStatus::from)?;
    let context = api
        .get_session_context(&auth_account_id, org_context)
        .await
        .map_err(|_| ErrorStatus::StatusCode(StatusCode::SERVICE_UNAVAILABLE))?;
    let instance_roles = context
        .instance_roles
        .into_iter()
        .map(|role| match role {
            InstanceRole::Admin => "admin".to_string(),
            InstanceRole::Moderator => "moderator".to_string(),
        })
        .collect();
    let org_context = context
        .org_context
        .map(|context| OrganizationContextResponse {
            org_account_id: AsRef::<i64>::as_ref(&context.org_account_id).to_string(),
            role: match context.role {
                kernel::prelude::entity::OrgRole::Owner => "owner".to_string(),
                kernel::prelude::entity::OrgRole::Admin => "admin".to_string(),
                kernel::prelude::entity::OrgRole::Member => "member".to_string(),
            },
        });
    Ok(Json(SessionContextResponse {
        auth_account_id: AsRef::<i64>::as_ref(&context.auth_account_id).to_string(),
        instance_roles,
        org_context,
    }))
}

#[cfg(test)]
#[path = "me_tests.rs"]
mod tests;
