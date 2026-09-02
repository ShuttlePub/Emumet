use crate::api::{resolve_auth_account_id, resolve_organization_context};
use crate::auth::{OidcAuthInfo, RequestOrganizationContext, ORGANIZATION_ID_HEADER};
use crate::error::ErrorStatus;
use crate::handler::AppModule;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;

pub(crate) async fn organization_context_middleware(
    State(app): State<AppModule>,
    mut request: Request<Body>,
    next: Next,
) -> Result<Response, ErrorStatus> {
    let organization_nanoid = request
        .headers()
        .get(ORGANIZATION_ID_HEADER)
        .map(|value| value.to_str())
        .transpose()
        .map_err(|_| axum::http::StatusCode::BAD_REQUEST)?;
    let context = match organization_nanoid {
        None => None,
        Some(organization_nanoid) => {
            let claims = request
                .extensions()
                .get::<crate::auth::AuthClaims>()
                .cloned()
                .ok_or(axum::http::StatusCode::UNAUTHORIZED)?;
            let auth_account_id = resolve_auth_account_id(&app, OidcAuthInfo::from(claims))
                .await
                .map_err(ErrorStatus::from)?;
            Some(
                resolve_organization_context(&app, &auth_account_id, organization_nanoid)
                    .await
                    .map_err(ErrorStatus::from)?,
            )
        }
    };
    request
        .extensions_mut()
        .insert(RequestOrganizationContext(context));
    Ok(next.run(request).await)
}
