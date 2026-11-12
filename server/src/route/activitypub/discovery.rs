use crate::error::ErrorStatus;
use axum::http::StatusCode;

/// A parsed `acct:name@domain` WebFinger resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WebFingerResource {
    pub account_name: String,
    pub domain: String,
}

pub(super) fn parse_webfinger_resource(resource: &str) -> Result<WebFingerResource, ErrorStatus> {
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
    Ok(WebFingerResource {
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
