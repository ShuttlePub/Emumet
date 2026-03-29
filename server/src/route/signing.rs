use crate::auth::{resolve_auth_account_id, AuthClaims, OidcAuthInfo};
use crate::error::ErrorStatus;
use crate::handler::AppModule;
use adapter::processor::account::{AccountQueryProcessor, DependOnAccountQueryProcessor};
use application::permission::{account_sign, check_permission};
use application::signing_key::{GetPublicKeyUseCase, SignRequestUseCase};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::prelude::entity::{Account, Nanoid};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::ToSchema;

#[derive(Debug, Deserialize, ToSchema)]
pub struct SignRequestBody {
    pub method: String,
    pub url: String,
    pub headers: HashMap<String, String>,
    /// Base64-encoded request body (optional)
    pub body: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SignResponse {
    pub cavage: HashMap<String, String>,
    pub rfc9421: HashMap<String, String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct PublicKeyResponse {
    pub id: String,
    pub owner: String,
    #[serde(rename = "publicKeyPem")]
    pub public_key_pem: String,
}

pub trait SigningAuthedRouter {
    fn route_signing_authed(self) -> Self;
}

pub trait SigningPublicRouter {
    fn route_signing_public(self) -> Self;
}

impl SigningAuthedRouter for Router<AppModule> {
    fn route_signing_authed(self) -> Self {
        self.route("/accounts/{id}/sign", post(sign_request))
    }
}

impl SigningPublicRouter for Router<AppModule> {
    fn route_signing_public(self) -> Self {
        self.route("/accounts/{id}/public-key", get(get_public_key))
    }
}

#[utoipa::path(
    post,
    path = "/accounts/{id}/sign",
    description = "Sign an HTTP request using the account's signing key.",
    params(("id" = String, Path, description = "Account nanoid")),
    request_body = SignRequestBody,
    responses(
        (status = 200, description = "Signed headers", body = SignResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Insufficient permissions"),
        (status = 404, description = "Account or signing key not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Signing",
)]
pub(crate) async fn sign_request(
    Extension(claims): Extension<AuthClaims>,
    State(module): State<AppModule>,
    Path(id): Path<String>,
    Json(request): Json<SignRequestBody>,
) -> Result<Json<SignResponse>, ErrorStatus> {
    if id.trim().is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Account ID cannot be empty".to_string(),
        )));
    }

    let auth_info = OidcAuthInfo::from(claims);
    let auth_account_id = resolve_auth_account_id(&module, auth_info)
        .await
        .map_err(ErrorStatus::from)?;

    let nanoid = Nanoid::<Account>::new(id);
    let mut executor = module
        .database_connection()
        .get_executor()
        .await
        .map_err(ErrorStatus::from)?;
    let account = module
        .account_query_processor()
        .find_by_nanoid(&mut executor, &nanoid)
        .await
        .map_err(ErrorStatus::from)?
        .ok_or_else(|| {
            ErrorStatus::from((StatusCode::NOT_FOUND, "Account not found".to_string()))
        })?;

    check_permission(&module, &auth_account_id, &account_sign(account.id()))
        .await
        .map_err(ErrorStatus::from)?;

    let body = match request.body {
        Some(ref b64) => {
            use base64::engine::general_purpose::STANDARD;
            use base64::Engine;
            let decoded = STANDARD.decode(b64).map_err(|e| {
                ErrorStatus::from((StatusCode::BAD_REQUEST, format!("Invalid base64 body: {e}")))
            })?;
            Some(decoded)
        }
        None => None,
    };

    let signing_request = kernel::interfaces::http_signing::HttpSigningRequest {
        method: request.method,
        url: request.url,
        headers: request.headers,
        body,
    };

    let response = module
        .sign(account.id(), signing_request)
        .await
        .map_err(ErrorStatus::from)?;

    Ok(Json(SignResponse {
        cavage: response.cavage_headers,
        rfc9421: response.rfc9421_headers,
    }))
}

#[utoipa::path(
    get,
    path = "/accounts/{id}/public-key",
    description = "Retrieve the public key for an account.",
    params(("id" = String, Path, description = "Account nanoid")),
    responses(
        (status = 200, description = "Public key info", body = PublicKeyResponse),
        (status = 404, description = "Account or signing key not found"),
    ),
    tag = "Signing",
)]
pub(crate) async fn get_public_key(
    State(module): State<AppModule>,
    Path(id): Path<String>,
) -> Result<Json<PublicKeyResponse>, ErrorStatus> {
    if id.trim().is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Account ID cannot be empty".to_string(),
        )));
    }

    let nanoid = Nanoid::<Account>::new(id);
    let mut executor = module
        .database_connection()
        .get_executor()
        .await
        .map_err(ErrorStatus::from)?;
    let account = module
        .account_query_processor()
        .find_by_nanoid(&mut executor, &nanoid)
        .await
        .map_err(ErrorStatus::from)?
        .ok_or_else(|| {
            ErrorStatus::from((StatusCode::NOT_FOUND, "Account not found".to_string()))
        })?;

    let info = module
        .get_public_key_info(account.id(), &nanoid)
        .await
        .map_err(ErrorStatus::from)?;

    Ok(Json(PublicKeyResponse {
        id: info.id,
        owner: info.owner,
        public_key_pem: info.public_key_pem,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_request_body_deserializes() {
        let json = r#"{
            "method": "POST",
            "url": "https://remote.example.com/inbox",
            "headers": {"content-type": "application/activity+json"},
            "body": "dGVzdCBib2R5"
        }"#;
        let body: SignRequestBody = serde_json::from_str(json).unwrap();
        assert_eq!(body.method, "POST");
        assert_eq!(body.url, "https://remote.example.com/inbox");
        assert_eq!(
            body.headers.get("content-type").unwrap(),
            "application/activity+json"
        );
        assert_eq!(body.body.as_deref(), Some("dGVzdCBib2R5"));
    }

    #[test]
    fn sign_request_body_without_body_deserializes() {
        let json = r#"{
            "method": "GET",
            "url": "https://remote.example.com/users/bob",
            "headers": {}
        }"#;
        let body: SignRequestBody = serde_json::from_str(json).unwrap();
        assert_eq!(body.method, "GET");
        assert!(body.body.is_none());
    }

    #[test]
    fn sign_response_serializes() {
        let mut cavage = HashMap::new();
        cavage.insert("signature".to_string(), "sig-value".to_string());
        let mut rfc9421 = HashMap::new();
        rfc9421.insert("signature".to_string(), "sig-value".to_string());

        let response = SignResponse { cavage, rfc9421 };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("cavage"));
        assert!(json.contains("rfc9421"));
    }

    #[test]
    fn public_key_response_serializes_with_camel_case() {
        let response = PublicKeyResponse {
            id: "https://example.com/accounts/abc#main-key".to_string(),
            owner: "https://example.com/accounts/abc".to_string(),
            public_key_pem: "-----BEGIN PUBLIC KEY-----\nMIIB...".to_string(),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("publicKeyPem"));
        assert!(!json.contains("public_key_pem"));
    }
}
