#![allow(dead_code)]

use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema)]
pub struct WebFingerResponse {
    pub subject: String,
    pub links: Option<Vec<WebFingerLink>>,
    pub aliases: Option<Vec<String>>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WebFingerLink {
    pub rel: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub href: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActorResponse {
    #[serde(rename = "@context")]
    #[schema(rename = "@context")]
    pub context: Vec<serde_json::Value>,
    pub id: String,
    #[serde(rename = "type")]
    pub object_type: String,
    pub preferred_username: String,
    pub name: Option<String>,
    pub summary: Option<String>,
    pub icon: Option<ImageObject>,
    pub inbox: String,
    pub outbox: String,
    pub followers: String,
    pub following: String,
    #[serde(rename = "publicKey")]
    pub public_key: PublicKey,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PublicKey {
    pub id: String,
    pub owner: String,
    pub public_key_pem: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImageObject {
    #[serde(rename = "type")]
    pub type_: String,
    pub url: Option<String>,
    pub media_type: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OrderedCollectionResponse {
    #[serde(rename = "@context")]
    #[schema(rename = "@context", value_type = Vec<String>)]
    pub context: Vec<String>,
    pub id: String,
    #[serde(rename = "type")]
    pub type_: String,
    #[serde(rename = "totalItems")]
    pub total_items: Option<u64>,
    pub first: Option<String>,
    pub last: Option<String>,
    #[serde(rename = "orderedItems")]
    pub ordered_items: Option<Vec<serde_json::Value>>,
}
