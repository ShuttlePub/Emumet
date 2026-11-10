//! Pure parsing of the Follow references carried by Accept / Reject / Undo activities.

use serde_json::Value;

/// URLs are compared modulo trailing slashes, like the Account inbox does.
pub(super) fn same_id(left: &str, right: &str) -> bool {
    left.trim_end_matches('/') == right.trim_end_matches('/')
}

/// `id` of an object reference: either the string itself or the `id` of an embedded object.
pub(super) fn reference_id(value: &Value) -> Option<String> {
    match value {
        Value::String(id) => Some(id.clone()),
        Value::Object(map) => map.get("id").and_then(Value::as_str).map(str::to_string),
        _ => None,
    }
}

/// The `object` of an Accept / Reject / Undo, classified as far as a Follow reference goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FollowObject {
    /// No usable `object` (absent, number, array ...).
    Missing,
    /// A bare activity ID. It may or may not be a Follow.
    Id(String),
    /// An embedded `Follow` object (any other embedded type is [`FollowObject::Other`]).
    Embedded {
        id: Option<String>,
        actor: Option<String>,
        object: Option<String>,
    },
    /// An embedded object of another type (Block, Offer, ...).
    Other,
}

pub(super) fn follow_object(object: Option<&Value>) -> FollowObject {
    match object {
        Some(Value::String(id)) => FollowObject::Id(id.clone()),
        Some(Value::Object(map)) => {
            if map.get("type").and_then(Value::as_str) != Some("Follow") {
                return FollowObject::Other;
            }
            FollowObject::Embedded {
                id: map.get("id").and_then(Value::as_str).map(str::to_string),
                actor: map.get("actor").and_then(reference_id),
                object: map.get("object").and_then(reference_id),
            }
        }
        _ => FollowObject::Missing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn same_id_ignores_trailing_slashes_only() {
        assert!(same_id("https://a.example/u/x", "https://a.example/u/x/"));
        assert!(!same_id("https://a.example/u/x", "https://a.example/u/y"));
        assert!(!same_id("https://a.example/u/x", "http://a.example/u/x"));
    }

    #[test]
    fn follow_object_classifies_every_shape() {
        assert_eq!(follow_object(None), FollowObject::Missing);
        assert_eq!(follow_object(Some(&json!(7))), FollowObject::Missing);
        assert_eq!(follow_object(Some(&json!([1]))), FollowObject::Missing);
        assert_eq!(
            follow_object(Some(&json!("https://r.example/f/1"))),
            FollowObject::Id("https://r.example/f/1".to_string())
        );
        assert_eq!(
            follow_object(Some(
                &json!({"type": "Block", "id": "https://r.example/b/1"})
            )),
            FollowObject::Other
        );
        assert_eq!(
            follow_object(Some(&json!({"id": "https://r.example/f/1"}))),
            FollowObject::Other
        );
        assert_eq!(
            follow_object(Some(&json!({
                "type": "Follow",
                "id": "https://r.example/f/1",
                "actor": "https://r.example/u/bob",
                "object": {"id": "https://l.example/ap/profiles/p"},
            }))),
            FollowObject::Embedded {
                id: Some("https://r.example/f/1".to_string()),
                actor: Some("https://r.example/u/bob".to_string()),
                object: Some("https://l.example/ap/profiles/p".to_string()),
            }
        );
        assert_eq!(
            follow_object(Some(&json!({"type": "Follow"}))),
            FollowObject::Embedded {
                id: None,
                actor: None,
                object: None
            }
        );
    }
}
