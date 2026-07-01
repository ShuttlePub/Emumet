//! ActivityStreams 2.0 serialization DTOs for ActivityPub protocol.
//!
//! This module provides serde-compatible DTOs for representing
//! ActivityPub actors, activities, collections, and related types.
//! These are domain-agnostic serialization models — no database or
//! business logic is included.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// JSON-LD Context
// ---------------------------------------------------------------------------

/// ActivityStreams JSON-LD context value.
///
/// Can be either a single URL string or an array of mixed values
/// (strings, objects) for extended contexts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Context {
    /// A single context URL string (e.g. `"https://www.w3.org/ns/activitystreams"`)
    Single(String),
    /// Multiple context entries (URLs and/or JSON objects for extensions)
    Multiple(Vec<serde_json::Value>),
}

// ---------------------------------------------------------------------------
// Actor (Person)
// ---------------------------------------------------------------------------

/// An ActivityPub Actor representing a local Account.
///
/// Serialized as a `Person` type with public key, inbox/outbox URLs,
/// and optional profile fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Actor {
    #[serde(rename = "@context")]
    pub context: Vec<serde_json::Value>,
    pub id: String,
    #[serde(rename = "type")]
    pub object_type: String,
    pub preferred_username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<ImageObject>,
    pub url: String,
    pub inbox: String,
    pub outbox: String,
    pub followers: String,
    pub following: String,
    #[serde(rename = "publicKey")]
    pub public_key: PublicKey,
}

impl Actor {
    /// Construct a new `Actor` from domain data.
    ///
    /// Automatically derives all ActivityPub endpoint URLs from the
    /// actor ID (`{base_url}/accounts/{account_nanoid}`).
    ///
    /// # Arguments
    ///
    /// * `base_url`           - Scheme + host, e.g. `"https://example.com"`
    /// * `account_nanoid`     - Unique identifier for the account
    /// * `preferred_username` - Account username (used as `preferredUsername`)
    /// * `display_name`       - Optional display name (used as `name`)
    /// * `summary`            - Optional profile bio/summary
    /// * `public_key_pem`     - PEM-encoded public key string
    /// * `public_key_id`      - Canonical URI for the public key
    pub fn new(
        base_url: &str,
        account_nanoid: &str,
        preferred_username: &str,
        display_name: Option<&str>,
        summary: Option<&str>,
        public_key_pem: &str,
        public_key_id: &str,
    ) -> Self {
        let base_url = base_url.trim_end_matches('/');
        let actor_id = format!("{}/accounts/{}", base_url, account_nanoid);
        let context = vec![
            serde_json::Value::String("https://www.w3.org/ns/activitystreams".to_string()),
            serde_json::json!({
                "publicKey": "https://w3id.org/security/v1#publicKey"
            }),
        ];

        Actor {
            context,
            id: actor_id.clone(),
            url: actor_id.clone(),
            object_type: "Person".to_string(),
            preferred_username: preferred_username.to_string(),
            name: display_name.map(|s| s.to_string()),
            summary: summary.map(|s| s.to_string()),
            icon: None,
            inbox: format!("{}/inbox", actor_id),
            outbox: format!("{}/outbox", actor_id),
            followers: format!("{}/followers", actor_id),
            following: format!("{}/following", actor_id),
            public_key: PublicKey {
                id: public_key_id.to_string(),
                owner: actor_id.clone(),
                public_key_pem: public_key_pem.to_string(),
            },
        }
    }
}

// ---------------------------------------------------------------------------
// PublicKey
// ---------------------------------------------------------------------------

/// An ActivityPub public key object attached to an Actor.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicKey {
    pub id: String,
    pub owner: String,
    pub public_key_pem: String,
}

// ---------------------------------------------------------------------------
// ImageObject
// ---------------------------------------------------------------------------

/// An ActivityPub Image object (e.g. profile avatar).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageObject {
    #[serde(rename = "type")]
    pub type_: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
}

// ---------------------------------------------------------------------------
// OrderedCollection
// ---------------------------------------------------------------------------

/// An ActivityPub `OrderedCollection` (e.g. followers, following, outbox).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderedCollection {
    #[serde(rename = "@context")]
    pub context: Vec<serde_json::Value>,
    pub id: String,
    #[serde(rename = "type")]
    pub type_: String,
    #[serde(rename = "totalItems", skip_serializing_if = "Option::is_none")]
    pub total_items: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last: Option<String>,
    #[serde(rename = "orderedItems", skip_serializing_if = "Option::is_none")]
    pub ordered_items: Option<Vec<serde_json::Value>>,
}

impl OrderedCollection {
    /// Create a new `OrderedCollection` with the standard ActivityStreams context.
    pub fn new(id: String, total_items: u64, first: Option<String>, last: Option<String>) -> Self {
        OrderedCollection {
            context: vec![serde_json::Value::String(
                "https://www.w3.org/ns/activitystreams".to_string(),
            )],
            id,
            type_: "OrderedCollection".to_string(),
            total_items: Some(total_items),
            first,
            last,
            ordered_items: None,
        }
    }

    pub fn with_ordered_items(
        id: String,
        total_items: u64,
        ordered_items: Vec<serde_json::Value>,
    ) -> Self {
        OrderedCollection {
            context: vec![serde_json::Value::String(
                "https://www.w3.org/ns/activitystreams".to_string(),
            )],
            id,
            type_: "OrderedCollection".to_string(),
            total_items: Some(total_items),
            first: None,
            last: None,
            ordered_items: Some(ordered_items),
        }
    }
}

// ---------------------------------------------------------------------------
// OrderedCollectionPage
// ---------------------------------------------------------------------------

/// A single page of an `OrderedCollection`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderedCollectionPage {
    #[serde(rename = "@context")]
    pub context: Vec<serde_json::Value>,
    pub id: String,
    #[serde(rename = "type")]
    pub type_: String,
    #[serde(rename = "partOf", skip_serializing_if = "Option::is_none")]
    pub part_of: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prev: Option<String>,
    pub ordered_items: Vec<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Activity (for inbox/outbox entries)
// ---------------------------------------------------------------------------

/// A generic ActivityPub Activity (e.g. Follow, Create, Like).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    #[serde(rename = "@context")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<serde_json::Value>,
    pub id: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub actor: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cc: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// WebFinger
// ---------------------------------------------------------------------------

/// A WebFinger response describing a resource and its links.
///
/// Both `links` and `aliases` are optional per RFC 7033 §4.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebFingerResponse {
    pub subject: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub links: Option<Vec<WebFingerLink>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aliases: Option<Vec<String>>,
}

/// A single link entry within a WebFinger response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebFingerLink {
    pub rel: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub href: String,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a minimal Actor for testing.
    fn test_actor() -> Actor {
        Actor::new(
            "https://example.com",
            "abc123",
            "alice",
            Some("Alice"),
            Some("Hello, I'm Alice!"),
            "-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA...\n-----END PUBLIC KEY-----",
            "https://example.com/accounts/abc123#main-key",
        )
    }

    // -----------------------------------------------------------------------
    // Actor serialization
    // -----------------------------------------------------------------------

    #[test]
    fn actor_serialization_uses_camelcase_fields() {
        let actor = test_actor();
        let json = serde_json::to_value(&actor).unwrap();
        let map = json.as_object().unwrap();

        // Special JSON-LD / ActivityStreams fields with custom rename
        assert!(map.contains_key("@context"), "missing @context");
        assert_eq!(map["type"], "Person");

        // camelCase fields via rename_all
        assert_eq!(map["preferredUsername"], "alice");
        assert_eq!(map["name"], "Alice");
        assert_eq!(map["summary"], "Hello, I'm Alice!");

        // publicKey has an explicit rename
        assert!(map.contains_key("publicKey"), "missing publicKey");

        // The publicKey sub-object should also use camelCase
        let pk = &map["publicKey"];
        assert!(pk.as_object().unwrap().contains_key("publicKeyPem"));
    }

    #[test]
    fn actor_includes_endpoint_urls() {
        let actor = test_actor();
        let json = serde_json::to_value(&actor).unwrap();

        assert_eq!(json["id"], "https://example.com/accounts/abc123");
        assert_eq!(json["inbox"], "https://example.com/accounts/abc123/inbox");
        assert_eq!(json["outbox"], "https://example.com/accounts/abc123/outbox");
        assert_eq!(
            json["followers"],
            "https://example.com/accounts/abc123/followers"
        );
        assert_eq!(
            json["following"],
            "https://example.com/accounts/abc123/following"
        );
    }

    #[test]
    fn actor_optional_fields_are_skipped_when_none() {
        let actor = Actor::new(
            "https://example.com",
            "xyz789",
            "bob",
            None, // display_name
            None, // summary
            "pem-content",
            "https://example.com/accounts/xyz789#main-key",
        );
        let json = serde_json::to_value(&actor).unwrap();
        let map = json.as_object().unwrap();

        assert!(!map.contains_key("name"), "name should be absent");
        assert!(!map.contains_key("summary"), "summary should be absent");
        assert!(!map.contains_key("icon"), "icon should be absent");
    }

    #[test]
    fn actor_public_key_structure() {
        let actor = test_actor();
        let json = serde_json::to_value(&actor).unwrap();
        let pk = &json["publicKey"];

        assert_eq!(pk["id"], "https://example.com/accounts/abc123#main-key");
        assert_eq!(pk["owner"], "https://example.com/accounts/abc123");
        assert!(pk["publicKeyPem"]
            .as_str()
            .unwrap()
            .starts_with("-----BEGIN PUBLIC KEY-----"));
    }

    // -----------------------------------------------------------------------
    // Deserialization: Follow activity
    // -----------------------------------------------------------------------

    #[test]
    fn deserialize_follow_activity_with_string_context() {
        // Test with single string @context (common in ActivityPub).
        // Our Activity.context uses serde_json::Value, which handles both
        // string and array forms.
        let raw = r#"{
            "@context": "https://www.w3.org/ns/activitystreams",
            "id": "https://example.com/activities/1",
            "type": "Follow",
            "actor": "https://remote.example/users/carol",
            "object": "https://example.com/accounts/alice",
            "to": ["https://example.com/accounts/alice"]
        }"#;

        let value: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(value["type"], "Follow");

        let activity: Activity = serde_json::from_value(value).unwrap();

        assert_eq!(activity.id, "https://example.com/activities/1");
        assert_eq!(activity.type_, "Follow");
        assert_eq!(activity.actor, "https://remote.example/users/carol");
        assert_eq!(
            activity.object,
            Some(serde_json::Value::String(
                "https://example.com/accounts/alice".to_string()
            ))
        );
        assert!(activity.target.is_none());
        assert!(activity.cc.is_none());

        // to field should have the single recipient
        let to = activity.to.unwrap();
        assert_eq!(to.len(), 1);
        assert_eq!(to[0], "https://example.com/accounts/alice");
    }

    // -----------------------------------------------------------------------
    // OrderedCollection serialization
    // -----------------------------------------------------------------------

    #[test]
    fn ordered_collection_serialization() {
        let collection = OrderedCollection::new(
            "https://example.com/accounts/alice/followers".to_string(),
            42,
            None,
            None,
        );

        let json = serde_json::to_value(&collection).unwrap();
        let map = json.as_object().unwrap();

        assert!(map.contains_key("@context"));
        assert_eq!(map["type"], "OrderedCollection");
        assert_eq!(map["totalItems"], 42);
        assert_eq!(map["id"], "https://example.com/accounts/alice/followers");
        assert!(!map.contains_key("first"));
        assert!(!map.contains_key("last"));
    }

    #[test]
    fn ordered_collection_first_last_optional() {
        let collection = OrderedCollection::new(
            "https://example.com/accounts/alice/following".to_string(),
            0,
            Some("https://example.com/accounts/alice/following?page=1".to_string()),
            None,
        );

        let json = serde_json::to_value(&collection).unwrap();
        let map = json.as_object().unwrap();

        assert_eq!(map["totalItems"], 0);
        assert_eq!(
            map["first"],
            "https://example.com/accounts/alice/following?page=1"
        );
        assert!(!map.contains_key("last"));
    }

    // -----------------------------------------------------------------------
    // OrderedCollectionPage serialization
    // -----------------------------------------------------------------------

    #[test]
    fn ordered_collection_page_serialization() {
        let page = OrderedCollectionPage {
            context: vec![serde_json::Value::String(
                "https://www.w3.org/ns/activitystreams".to_string(),
            )],
            id: "https://example.com/accounts/alice/followers?page=1".to_string(),
            type_: "OrderedCollectionPage".to_string(),
            part_of: Some("https://example.com/accounts/alice/followers".to_string()),
            next: Some("https://example.com/accounts/alice/followers?page=2".to_string()),
            prev: None,
            ordered_items: vec![
                serde_json::json!("https://remote.example/users/carol"),
                serde_json::json!("https://remote.example/users/dave"),
            ],
        };

        let json = serde_json::to_value(&page).unwrap();
        let map = json.as_object().unwrap();

        assert_eq!(map["type"], "OrderedCollectionPage");
        assert_eq!(map["orderedItems"].as_array().unwrap().len(), 2);
        assert_eq!(
            map["partOf"],
            "https://example.com/accounts/alice/followers"
        );
        assert!(map.contains_key("next"));
        assert!(!map.contains_key("prev"));
    }

    // -----------------------------------------------------------------------
    // WebFinger serialization
    // -----------------------------------------------------------------------

    #[test]
    fn webfinger_response_serialization() {
        let response = WebFingerResponse {
            subject: "acct:alice@example.com".to_string(),
            links: Some(vec![WebFingerLink {
                rel: "self".to_string(),
                type_: "application/activity+json".to_string(),
                href: "https://example.com/accounts/alice".to_string(),
            }]),
            aliases: None,
        };

        let json = serde_json::to_value(&response).unwrap();
        let map = json.as_object().unwrap();

        assert_eq!(map["subject"], "acct:alice@example.com");
        assert_eq!(map["links"].as_array().unwrap().len(), 1);

        let link = &map["links"][0];
        assert_eq!(link["rel"], "self");
        assert_eq!(link["type"], "application/activity+json");
        assert_eq!(link["href"], "https://example.com/accounts/alice");
    }

    // -----------------------------------------------------------------------
    // Round-trip: serialize → deserialize
    // -----------------------------------------------------------------------

    #[test]
    fn actor_round_trip() {
        let original = test_actor();
        let json = serde_json::to_value(&original).unwrap();
        let deserialized: Actor = serde_json::from_value(json).unwrap();

        assert_eq!(deserialized.id, original.id);
        assert_eq!(deserialized.object_type, "Person");
        assert_eq!(deserialized.preferred_username, original.preferred_username);
        assert_eq!(deserialized.name, original.name);
        assert_eq!(deserialized.summary, original.summary);
        assert_eq!(
            deserialized.public_key.public_key_pem,
            original.public_key.public_key_pem
        );
    }

    #[test]
    fn ordered_collection_round_trip() {
        let original = OrderedCollection::new(
            "https://example.com/accounts/alice/followers".to_string(),
            10,
            None,
            None,
        );
        let json = serde_json::to_value(&original).unwrap();
        let deserialized: OrderedCollection = serde_json::from_value(json).unwrap();

        assert_eq!(deserialized.id, original.id);
        assert_eq!(deserialized.type_, "OrderedCollection");
        assert_eq!(deserialized.total_items, Some(10));
    }

    // -----------------------------------------------------------------------
    // ImageObject serialization
    // -----------------------------------------------------------------------

    #[test]
    fn image_object_serialization() {
        let image = ImageObject {
            type_: "Image".to_string(),
            url: Some("https://example.com/media/avatar.png".to_string()),
            media_type: Some("image/png".to_string()),
        };

        let json = serde_json::to_value(&image).unwrap();
        let map = json.as_object().unwrap();

        assert_eq!(map["type"], "Image");
        assert_eq!(map["url"], "https://example.com/media/avatar.png");
        assert_eq!(map["mediaType"], "image/png");
    }

    #[test]
    fn image_object_optional_url() {
        let image = ImageObject {
            type_: "Image".to_string(),
            url: None,
            media_type: None,
        };

        let json = serde_json::to_value(&image).unwrap();
        let map = json.as_object().unwrap();

        assert!(!map.contains_key("url"));
        assert!(!map.contains_key("mediaType"));
    }

    // -----------------------------------------------------------------------
    // Activity serialization
    // -----------------------------------------------------------------------

    #[test]
    fn activity_serialization() {
        let activity = Activity {
            context: Some(serde_json::json!(["https://www.w3.org/ns/activitystreams"])),
            id: "https://example.com/activities/2".to_string(),
            type_: "Create".to_string(),
            actor: "https://example.com/accounts/alice".to_string(),
            object: Some(serde_json::json!({
                "id": "https://example.com/posts/1",
                "type": "Note",
                "content": "Hello world!"
            })),
            target: None,
            to: Some(vec![
                "https://www.w3.org/ns/activitystreams#Public".to_string()
            ]),
            cc: Some(vec![
                "https://example.com/accounts/alice/followers".to_string()
            ]),
        };

        let json = serde_json::to_value(&activity).unwrap();
        let map = json.as_object().unwrap();

        assert_eq!(map["type"], "Create");
        assert_eq!(map["actor"], "https://example.com/accounts/alice");
        assert_eq!(map["object"]["type"], "Note");
        assert!(map.contains_key("to"));
        assert!(map.contains_key("cc"));
    }

    // -----------------------------------------------------------------------
    // Regression tests for Cycle 1 fixes
    // -----------------------------------------------------------------------

    #[test]
    fn actor_new_strips_trailing_slash() {
        let actor = Actor::new(
            "https://example.com/",
            "abc123",
            "test",
            None,
            None,
            "pem",
            "https://example.com/accounts/abc123#main-key",
        );
        assert_eq!(
            actor.id, "https://example.com/accounts/abc123",
            "trailing slash in base_url should be stripped"
        );
        assert_eq!(actor.inbox, "https://example.com/accounts/abc123/inbox");
    }

    #[test]
    fn activity_deserialize_with_array_context() {
        // Array @context should also deserialize correctly.
        let raw = r#"{
            "@context": ["https://www.w3.org/ns/activitystreams", {"emoji": "https://example.com/ns/emoji"}],
            "id": "https://example.com/activities/3",
            "type": "Like",
            "actor": "https://remote.example/users/carol",
            "object": "https://example.com/posts/1"
        }"#;

        let activity: Activity = serde_json::from_str(raw).unwrap();
        assert_eq!(activity.type_, "Like");
        assert!(activity.to.is_none());
        assert!(activity.cc.is_none());

        // Context should be deserialized as a JSON array value.
        let ctx = activity.context.unwrap();
        assert!(ctx.is_array());
        assert_eq!(ctx[0], "https://www.w3.org/ns/activitystreams");
    }

    #[test]
    fn webfinger_response_links_optional() {
        // Per RFC 7033, a JRD with no links is valid.
        let response = WebFingerResponse {
            subject: "acct:bob@example.com".to_string(),
            links: None,
            aliases: Some(vec!["https://example.com/accounts/bob".to_string()]),
        };

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["subject"], "acct:bob@example.com");
        assert!(
            json.get("links").is_none(),
            "links should not appear when None"
        );
        assert_eq!(json["aliases"][0], "https://example.com/accounts/bob");
    }
}
