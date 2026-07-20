use super::{json_response, public_base_host_header, JRD_JSON};
use crate::error::ErrorStatus;
use crate::handler::AppModule;
use application::service::activitypub::GetWebFingerUseCase;
use application::transfer::activitypub::GetWebFingerDto;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use kernel::interfaces::config::DependOnPublicBaseUrl;
use std::collections::HashMap;

#[utoipa::path(
    get,
    path = "/.well-known/webfinger",
    description = "WebFinger account discovery for ActivityPub.",
    params(("resource" = String, Query, description = "Resource URI (acct:user@domain)")),
    responses(
        (status = 200, description = "WebFinger response", body = kernel::activitypub::WebFingerResponse, content_type = "application/jrd+json"),
        (status = 400, description = "Invalid resource format"),
        (status = 404, description = "Account not found"),
    ),
    tag = "ActivityPub",
)]
pub(crate) async fn webfinger(
    State(module): State<AppModule>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Response, ErrorStatus> {
    let resource = params.get("resource").ok_or_else(|| {
        ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Missing resource parameter".to_string(),
        ))
    })?;
    let dto = parse_webfinger_resource(resource)?;
    let expected_domain = public_base_host_header(module.public_base_url().as_str())?;
    if !dto.domain.eq_ignore_ascii_case(&expected_domain) {
        tracing::debug!(resource, expected_domain, "WebFinger domain mismatch");
        return Err(ErrorStatus::from(StatusCode::NOT_FOUND));
    }
    let response = module.get_webfinger(dto).await.map_err(ErrorStatus::from)?;

    json_response(&response, JRD_JSON)
}

fn parse_webfinger_resource(resource: &str) -> Result<GetWebFingerDto, ErrorStatus> {
    let value = resource
        .strip_prefix("acct:")
        .ok_or_else(invalid_resource)?;
    let (account_name, domain) = value.split_once('@').ok_or_else(invalid_resource)?;
    if account_name.trim().is_empty()
        || domain.trim().is_empty()
        || account_name.contains(char::is_whitespace)
        || domain.contains(char::is_whitespace)
        || domain.contains('@')
    {
        return Err(invalid_resource());
    }
    Ok(GetWebFingerDto {
        account_name: account_name.to_string(),
        domain: domain.to_string(),
    })
}

fn invalid_resource() -> ErrorStatus {
    ErrorStatus::from((
        StatusCode::BAD_REQUEST,
        "Invalid WebFinger resource format".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_webfinger_resource_accepts_acct_uri() {
        let dto = parse_webfinger_resource("acct:alice@example.com").unwrap();
        assert_eq!(dto.account_name, "alice");
        assert_eq!(dto.domain, "example.com");
    }

    #[test]
    fn parse_webfinger_resource_rejects_invalid_values() {
        for resource in [
            "alice@example.com",
            "acct:alice",
            "acct:@example.com",
            "acct:alice@",
            "acct:ali ce@example.com",
            "acct:alice@example.com@other.example",
        ] {
            assert!(parse_webfinger_resource(resource).is_err(), "{resource}");
        }
    }
}
