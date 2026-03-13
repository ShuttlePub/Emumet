use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct LoginQuery {
    pub login_challenge: String,
}

#[derive(Debug, Deserialize)]
pub struct ConsentQuery {
    pub consent_challenge: String,
}

#[derive(Debug, Serialize)]
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

#[derive(Debug, Deserialize)]
pub struct ConsentDecision {
    pub consent_challenge: String,
    pub accept: bool,
    pub grant_scope: Option<Vec<String>>,
}
