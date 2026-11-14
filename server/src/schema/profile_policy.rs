//! Profile identity / relationship / domain-controls schemas (E8 contract, served since the P4
//! cutover; the domain-controls routes since D8R). All external IDs are nanoids.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;

/// Request to follow a target from a Profile.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateProfileFollowRequest {
    /// Remote actor URL (e.g. https://remote.example/users/bob), `acct:name@domain`,
    /// or a local Profile nanoid.
    pub target: String,
}

/// Lifecycle state of a Profile follow.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProfileFollowState {
    /// Follow sent / recorded, awaiting the target's Accept.
    Pending,
    /// Follow accepted by the target.
    Accepted,
}

/// A Profile-scoped follow relationship (the Profile is the follower).
#[derive(Debug, Serialize, ToSchema)]
pub struct ProfileFollowResponse {
    /// Follow relation nanoid.
    pub id: String,
    /// Follower Profile nanoid.
    pub profile_id: String,
    /// Canonical actor URL of the followed target.
    pub target: String,
    pub state: ProfileFollowState,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// Profiles/actors followed by a Profile (the "following" list).
#[derive(Debug, Serialize, ToSchema)]
pub struct ProfileFollowListResponse {
    pub items: Vec<ProfileFollowResponse>,
    /// Cursor for the next page; absent on the last page.
    pub next_cursor: Option<String>,
}

/// Reference to a follower or target actor, local or remote.
#[derive(Debug, Serialize, ToSchema)]
pub struct ProfileActorRef {
    pub kind: ProfileActorKind,
    /// Canonical ActivityPub actor ID (URL). Always present, for local and remote actors alike.
    pub actor_url: String,
    /// Profile nanoid; present only when kind is `local`, absent for remote actors.
    pub profile_id: Option<String>,
}

/// Whether an actor lives on this server or on a remote server.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProfileActorKind {
    /// A Profile hosted by this server.
    Local,
    /// A remote ActivityPub actor identified only by its canonical actor URL.
    Remote,
}

/// A follow relationship in which the Profile is the followed party (the follower is the actor).
#[derive(Debug, Serialize, ToSchema)]
pub struct ProfileFollowerResponse {
    /// Follow relation nanoid.
    pub id: String,
    /// Followed Profile nanoid (the owner of the list).
    pub profile_id: String,
    /// The follower: a local Profile or a remote actor.
    pub follower: ProfileActorRef,
    pub state: ProfileFollowState,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// Actors following a Profile (the "followers" list).
#[derive(Debug, Serialize, ToSchema)]
pub struct ProfileFollowerListResponse {
    pub items: Vec<ProfileFollowerResponse>,
    /// Cursor for the next page; absent on the last page.
    pub next_cursor: Option<String>,
}

/// Request to block or mute one target (individual actor) from a Profile.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateProfileRelationRequest {
    /// Remote actor URL, `acct:name@domain`, or a local Profile nanoid.
    pub target: String,
}

/// Kind of individual (non-domain) relation setting.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProfileRelationKind {
    /// Suppress display only; existing relationships are kept.
    Mute,
    /// Suppress display, detach existing follow relations in both directions and
    /// reject new interactions. Removing a block does not restore detached relations.
    Block,
}

/// An individual block or mute setting owned by one Profile.
#[derive(Debug, Serialize, ToSchema)]
pub struct ProfileRelationResponse {
    /// Relation setting nanoid.
    pub id: String,
    /// Owning Profile nanoid.
    pub profile_id: String,
    pub kind: ProfileRelationKind,
    /// Canonical actor URL of the target.
    pub target: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    /// Display information of the target actor (name, icon, acct) for the account list.
    /// Never contacts the remote server while serving this response; see
    /// `ProfileRelationActor.status` for the fallback.
    pub target_actor: ProfileRelationActor,
}

/// Whether the target of a block or mute lives on this server or a remote server.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProfileRelationActorKind {
    /// A Profile hosted by this server.
    Local,
    /// A remote ActivityPub actor.
    Remote,
}

/// Whether display information of a target actor is available.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProfileRelationActorStatus {
    /// Display information is available (`acct` and `preferred_username` are set; `display_name` and
    /// `icon_url` are null when the actor has none). A local target is always resolved. A remote
    /// target whose earlier fetch succeeded stays resolved with the old data even when a later
    /// refresh fails.
    Resolved,
    /// A remote actor that has not been fetched yet (a fetch is scheduled or running). All display
    /// fields are null; read the list again later. Fall back to `target`.
    Pending,
    /// A remote actor whose last fetch failed (unreachable, gone, malformed). All display fields are
    /// null and the fetch is retried later. Fall back to `target`.
    Unavailable,
}

/// Display information of the target of a block or mute, served from the Profile (local) or a
/// server-side cache of the remote actor document (remote). Text fields are plain text, never HTML.
#[derive(Debug, Serialize, ToSchema)]
pub struct ProfileRelationActor {
    pub kind: ProfileRelationActorKind,
    /// Canonical actor URL; always equal to `target`.
    pub actor_url: String,
    /// Profile nanoid; present only when kind is `local`.
    pub profile_id: Option<String>,
    pub status: ProfileRelationActorStatus,
    /// `preferred_username@host` without a leading `@` (host includes a non-default port).
    pub acct: Option<String>,
    pub preferred_username: Option<String>,
    pub display_name: Option<String>,
    /// Absolute http(s) URL of the avatar as published by the actor (remote) or stored for the
    /// Profile (local). The browser loads it directly.
    pub icon_url: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ProfileRelationListResponse {
    pub items: Vec<ProfileRelationResponse>,
    /// Cursor for the next page; absent on the last page.
    pub next_cursor: Option<String>,
}

/// Treatment applied to every actor whose canonical actor-ID host equals the domain.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DomainControlMode {
    /// Display suppression only (timeline, search, notifications). Relationships are kept.
    Mute,
    /// Display suppression, bidirectional follow detachment and rejection of new
    /// follows. No ActivityPub Block is sent. Removing the block does not restore relations.
    Block,
}

/// A Profile x exact-match domain control.
///
/// Matching is exact: `example.com` does not cover `sub.example.com`.
/// Settings are per Profile and never propagate to other Profiles or Accounts.
#[derive(Debug, Serialize, ToSchema)]
pub struct DomainControlResponse {
    /// Owning Profile nanoid.
    pub profile_id: String,
    /// Normalized domain (lowercase ASCII / IDNA A-label host, no scheme, port or wildcard).
    pub domain: String,
    pub mode: DomainControlMode,
    /// Monotonic revision starting at 1, incremented on every change.
    /// Send it back as `expected_revision` to update or delete safely.
    pub revision: u64,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DomainControlListResponse {
    pub items: Vec<DomainControlResponse>,
}

/// Create or replace the control for one domain.
#[derive(Debug, Deserialize, ToSchema)]
pub struct PutDomainControlRequest {
    pub mode: DomainControlMode,
    /// Optimistic concurrency token.
    /// Omit (or null) to create: 409 if a control for the domain already exists.
    /// Set to the current `revision` to replace: 409 if it no longer matches
    /// (including when the control was deleted meanwhile).
    #[serde(default)]
    pub expected_revision: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn created_at() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap()
    }

    #[test]
    fn follower_fixture_represents_a_remote_actor_by_canonical_url() {
        let follower = ProfileFollowerResponse {
            id: "follow-1".to_string(),
            profile_id: "profile-1".to_string(),
            follower: ProfileActorRef {
                kind: ProfileActorKind::Remote,
                actor_url: "https://remote.example/users/bob".to_string(),
                profile_id: None,
            },
            state: ProfileFollowState::Accepted,
            created_at: created_at(),
        };
        assert_eq!(
            serde_json::to_value(&follower).unwrap(),
            serde_json::json!({
                "id": "follow-1",
                "profile_id": "profile-1",
                "follower": {
                    "kind": "remote",
                    "actor_url": "https://remote.example/users/bob",
                    "profile_id": null,
                },
                "state": "accepted",
                "created_at": "2023-11-14T22:13:20Z",
            })
        );
    }

    #[test]
    fn follower_fixture_represents_a_local_profile_with_its_nanoid() {
        let follower = ProfileFollowerResponse {
            id: "follow-2".to_string(),
            profile_id: "profile-1".to_string(),
            follower: ProfileActorRef {
                kind: ProfileActorKind::Local,
                actor_url: "https://emumet.example/ap/profiles/profile-2".to_string(),
                profile_id: Some("profile-2".to_string()),
            },
            state: ProfileFollowState::Pending,
            created_at: created_at(),
        };
        let json = serde_json::to_value(&follower).unwrap();
        assert_eq!(json["follower"]["kind"], "local");
        assert_eq!(
            json["follower"]["actor_url"],
            "https://emumet.example/ap/profiles/profile-2"
        );
        assert_eq!(json["follower"]["profile_id"], "profile-2");
        assert_eq!(json["state"], "pending");
    }

    #[test]
    fn follower_list_mixes_local_and_remote_followers() {
        let list = ProfileFollowerListResponse {
            items: vec![
                ProfileFollowerResponse {
                    id: "f1".to_string(),
                    profile_id: "p".to_string(),
                    follower: ProfileActorRef {
                        kind: ProfileActorKind::Remote,
                        actor_url: "https://remote.example/users/bob".to_string(),
                        profile_id: None,
                    },
                    state: ProfileFollowState::Accepted,
                    created_at: created_at(),
                },
                ProfileFollowerResponse {
                    id: "f2".to_string(),
                    profile_id: "p".to_string(),
                    follower: ProfileActorRef {
                        kind: ProfileActorKind::Local,
                        actor_url: "https://emumet.example/ap/profiles/q".to_string(),
                        profile_id: Some("q".to_string()),
                    },
                    state: ProfileFollowState::Accepted,
                    created_at: created_at(),
                },
            ],
            next_cursor: None,
        };
        let json = serde_json::to_value(&list).unwrap();
        let kinds: Vec<_> = json["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["follower"]["kind"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["remote", "local"]);
        assert!(json["next_cursor"].is_null());
    }
}
