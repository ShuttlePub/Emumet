//! Profile 配送 job の本文 (ActivityPub activity の JSON) の組み立て。純粋関数で、DB・HTTP に依存しない。
//!
//! 本文は job にバイト列として保存され、HTTP 署名の digest もそのバイト列から作られる。
//! activity ID は決定的 (Follow は Follow 世代 ID、Undo / Reject は元の Follow から導出) なので、
//! 同じ関係の再処理は冪等キー `(sender, activity_id, target_inbox)` で重複 job にならない。
//! AP `Block` は **個別 actor の block だけ**が組み立てる (ユーザー決定。解除は元 Block を埋め込んだ
//! `Undo(Block)`)。domain block は `Block` を作らない。

use error_stack::Report;
use kernel::activitypub::ActorUrlBuilder;
use kernel::interfaces::config::PublicBaseUrl;
use kernel::prelude::entity::{GraphActivityId, ProfileBlockId, ProfileFollowId};
use kernel::KernelError;
use serde_json::{json, Value};

const ACTIVITYSTREAMS_CONTEXT: &str = "https://www.w3.org/ns/activitystreams";

/// ローカル Profile の actor URL (`{PUBLIC_BASE_URL}/ap/profiles/{nanoid}`)。
pub fn profile_actor_url(public_base_url: &PublicBaseUrl, profile_nanoid: &str) -> String {
    ActorUrlBuilder::for_profile(public_base_url.as_str(), profile_nanoid).actor_id()
}

/// ローカル Profile の inbox URL。
pub fn profile_inbox_url(public_base_url: &PublicBaseUrl, profile_nanoid: &str) -> String {
    ActorUrlBuilder::for_profile(public_base_url.as_str(), profile_nanoid).inbox()
}

/// `{PUBLIC_BASE_URL}/ap/profiles/{nanoid}` の形ならその nanoid を返す (ローカル Profile の actor URL)。
pub fn local_profile_nanoid<'a>(
    public_base_url: &PublicBaseUrl,
    actor_url: &'a str,
) -> Option<&'a str> {
    let prefix = format!("{}/ap/profiles/", public_base_url.as_str());
    let rest = actor_url.strip_prefix(prefix.as_str())?;
    (!rest.is_empty() && !rest.contains(['/', '?', '#'])).then_some(rest)
}

fn validation(message: &str) -> Report<KernelError> {
    Report::new(KernelError::Internal).attach_printable(message.to_string())
}

fn activity_id(value: String) -> error_stack::Result<GraphActivityId, KernelError> {
    GraphActivityId::new(value)
}

/// Follow activity の ID。Follow 世代 ID から決まる。
pub fn follow_activity_id(
    public_base_url: &PublicBaseUrl,
    sender_nanoid: &str,
    follow_id: &ProfileFollowId,
) -> error_stack::Result<GraphActivityId, KernelError> {
    activity_id(format!(
        "{}/activities/{}",
        profile_actor_url(public_base_url, sender_nanoid),
        follow_id.as_ref()
    ))
}

/// 元 Follow を参照する Undo の ID。元 Follow の ID から決まる (同じ Follow への Undo は常に同じ ID)。
pub fn undo_activity_id(
    follow_activity_id: &GraphActivityId,
) -> error_stack::Result<GraphActivityId, KernelError> {
    activity_id(format!("{}/undo", follow_activity_id.as_str()))
}

/// 個別 block の `Block` activity の ID。block ID から決まる (同じ block の再処理は冪等キーで重複 job にならない)。
pub fn block_activity_id(
    public_base_url: &PublicBaseUrl,
    sender_nanoid: &str,
    block_id: &ProfileBlockId,
) -> error_stack::Result<GraphActivityId, KernelError> {
    activity_id(format!(
        "{}/blocks/{}",
        profile_actor_url(public_base_url, sender_nanoid),
        block_id.as_ref()
    ))
}

/// Follow 世代に対する Reject の ID。
pub fn reject_activity_id(
    public_base_url: &PublicBaseUrl,
    sender_nanoid: &str,
    follow_id: &ProfileFollowId,
) -> error_stack::Result<GraphActivityId, KernelError> {
    activity_id(format!(
        "{}/activities/{}/reject",
        profile_actor_url(public_base_url, sender_nanoid),
        follow_id.as_ref()
    ))
}

fn to_body(value: Value) -> error_stack::Result<String, KernelError> {
    serde_json::to_string(&value).map_err(|_| validation("failed to serialize activity"))
}

/// `Follow` activity の本文。
pub fn follow_body(
    id: &GraphActivityId,
    actor: &str,
    target: &str,
) -> error_stack::Result<String, KernelError> {
    to_body(json!({
        "@context": ACTIVITYSTREAMS_CONTEXT,
        "id": id.as_str(),
        "type": "Follow",
        "actor": actor,
        "object": target,
        "to": [target],
    }))
}

/// `Block` activity の本文 (`object` = 相手 actor)。
pub fn block_body(
    id: &GraphActivityId,
    actor: &str,
    target: &str,
) -> error_stack::Result<String, KernelError> {
    to_body(json!({
        "@context": ACTIVITYSTREAMS_CONTEXT,
        "id": id.as_str(),
        "type": "Block",
        "actor": actor,
        "object": target,
        "to": [target],
    }))
}

/// 元 Block の本文 (JSON) を `object` に埋め込んだ `Undo(Block)` の本文 (`@context` は埋め込みから除く)。
pub fn undo_block_body(
    id: &GraphActivityId,
    actor: &str,
    target: &str,
    original_block_body: &str,
) -> error_stack::Result<String, KernelError> {
    undo_follow_body(id, actor, target, original_block_body)
}

/// 元 Follow の本文 (JSON) を `object` に埋め込んだ `Undo(Follow)` の本文。
/// 元 Follow の `@context` は埋め込みから除く。
pub fn undo_follow_body(
    id: &GraphActivityId,
    actor: &str,
    target: &str,
    original_follow_body: &str,
) -> error_stack::Result<String, KernelError> {
    let mut original: Value = serde_json::from_str(original_follow_body)
        .map_err(|_| validation("stored follow payload is not JSON"))?;
    if let Some(object) = original.as_object_mut() {
        object.remove("@context");
    }
    to_body(json!({
        "@context": ACTIVITYSTREAMS_CONTEXT,
        "id": id.as_str(),
        "type": "Undo",
        "actor": actor,
        "object": original,
        "to": [target],
    }))
}

/// 相手が送った Follow (`follow_activity_id`, `follower` → `actor`) を拒否・解除する `Reject(Follow)` の本文。
pub fn reject_follow_body(
    id: &GraphActivityId,
    actor: &str,
    follower: &str,
    follow_activity_id: &GraphActivityId,
) -> error_stack::Result<String, KernelError> {
    to_body(json!({
        "@context": ACTIVITYSTREAMS_CONTEXT,
        "id": id.as_str(),
        "type": "Reject",
        "actor": actor,
        "object": {
            "id": follow_activity_id.as_str(),
            "type": "Follow",
            "actor": follower,
            "object": actor,
        },
        "to": [follower],
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> PublicBaseUrl {
        PublicBaseUrl::new("https://local.example/".to_string())
    }

    #[test]
    fn local_profile_actor_urls_are_recognised_exactly() {
        let base = base();
        assert_eq!(
            local_profile_nanoid(&base, "https://local.example/ap/profiles/abc"),
            Some("abc")
        );
        for other in [
            "https://local.example/ap/profiles/",
            "https://local.example/ap/profiles/abc/inbox",
            "https://local.example/ap/profiles/abc#main-key",
            "https://local.example/ap/accounts/abc",
            "https://remote.example/ap/profiles/abc",
            "http://local.example/ap/profiles/abc",
        ] {
            assert_eq!(local_profile_nanoid(&base, other), None, "{other}");
        }
    }

    #[test]
    fn activity_ids_are_deterministic_and_derived() {
        let base = base();
        let follow_id = ProfileFollowId::new(42);
        let follow = follow_activity_id(&base, "me", &follow_id).unwrap();
        assert_eq!(
            follow.as_str(),
            "https://local.example/ap/profiles/me/activities/42"
        );
        assert_eq!(
            undo_activity_id(&follow).unwrap().as_str(),
            "https://local.example/ap/profiles/me/activities/42/undo"
        );
        assert_eq!(
            reject_activity_id(&base, "me", &follow_id)
                .unwrap()
                .as_str(),
            "https://local.example/ap/profiles/me/activities/42/reject"
        );
    }

    #[test]
    fn undo_embeds_the_original_follow_without_context() {
        let base = base();
        let follow_id = follow_activity_id(&base, "me", &ProfileFollowId::new(7)).unwrap();
        let body = follow_body(
            &follow_id,
            "https://local.example/ap/profiles/me",
            "https://remote.example/users/bob",
        )
        .unwrap();
        let undo_id = undo_activity_id(&follow_id).unwrap();
        let undo = undo_follow_body(
            &undo_id,
            "https://local.example/ap/profiles/me",
            "https://remote.example/users/bob",
            &body,
        )
        .unwrap();
        let value: Value = serde_json::from_str(&undo).unwrap();
        assert_eq!(value["type"], "Undo");
        assert_eq!(value["object"]["type"], "Follow");
        assert_eq!(value["object"]["id"], follow_id.as_str());
        assert_eq!(
            value["object"]["actor"],
            "https://local.example/ap/profiles/me"
        );
        assert_eq!(
            value["object"]["object"],
            "https://remote.example/users/bob"
        );
        assert!(value["object"].get("@context").is_none());
        assert_eq!(value["to"][0], "https://remote.example/users/bob");
    }

    #[test]
    fn reject_references_the_followers_original_follow() {
        let id = GraphActivityId::new("https://local.example/ap/profiles/me/activities/1/reject")
            .unwrap();
        let original = GraphActivityId::new("https://remote.example/follows/9").unwrap();
        let body = reject_follow_body(
            &id,
            "https://local.example/ap/profiles/me",
            "https://remote.example/users/bob",
            &original,
        )
        .unwrap();
        let value: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value["type"], "Reject");
        assert_eq!(value["object"]["id"], "https://remote.example/follows/9");
        assert_eq!(value["object"]["actor"], "https://remote.example/users/bob");
        assert_eq!(
            value["object"]["object"],
            "https://local.example/ap/profiles/me"
        );
    }
}
