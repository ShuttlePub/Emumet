use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Deserialize, ToSchema)]
pub struct LoginQuery {
    pub login_challenge: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ConsentQuery {
    pub consent_challenge: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "action")]
pub enum OAuth2Response {
    #[serde(rename = "redirect")]
    Redirect { redirect_to: String },
    #[serde(rename = "show_consent")]
    ShowConsent {
        consent_challenge: String,
        client_name: Option<String>,
        requested_scope: Vec<String>,
    },
}

impl IntoResponse for OAuth2Response {
    fn into_response(self) -> axum::response::Response {
        match self {
            OAuth2Response::Redirect { redirect_to } => (
                StatusCode::FOUND,
                [(axum::http::header::LOCATION, redirect_to)],
            )
                .into_response(),
            show_consent @ OAuth2Response::ShowConsent { .. } => Json(show_consent).into_response(),
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ConsentDecision {
    pub consent_challenge: String,
    pub accept: bool,
    pub grant_scope: Option<Vec<String>>,
}
