use crate::api::SigningApi;
use crate::auth::{AuthClaims, OidcAuthInfo};
use crate::error::ErrorStatus;
use crate::handler::AppModule;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
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

pub trait SigningRouter {
    fn route_signing(self) -> Self;
}

impl SigningRouter for Router<AppModule> {
    fn route_signing(self) -> Self {
        self.route("/accounts/{id}/sign", post(sign_request))
            .route("/accounts/{id}/public-key", get(get_public_key))
    }
}

#[utoipa::path(
    post,
    path = "/internal/v1/accounts/{id}/sign",
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
    State(api): State<SigningApi>,
    Path(id): Path<String>,
    Json(request): Json<SignRequestBody>,
) -> Result<Json<SignResponse>, ErrorStatus> {
    let auth_account_id = api
        .resolve_auth_account_id(OidcAuthInfo::from(claims))
        .await
        .map_err(ErrorStatus::from)?;

    let account = api.find_account_by_nanoid(id).await?;

    api.check_sign_permission(&auth_account_id, account.id())
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

    let response = api
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
    path = "/internal/v1/accounts/{id}/public-key",
    description = "Retrieve the public key for an account.",
    params(("id" = String, Path, description = "Account nanoid")),
    responses(
        (status = 200, description = "Public key info", body = PublicKeyResponse),
        (status = 404, description = "Account or signing key not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Signing",
)]
pub(crate) async fn get_public_key(
    State(api): State<SigningApi>,
    Path(id): Path<String>,
) -> Result<Json<PublicKeyResponse>, ErrorStatus> {
    let account = api.find_account_by_nanoid(id.clone()).await?;

    let info = api
        .get_public_key_info(account.id(), &kernel::prelude::entity::Nanoid::new(id))
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
