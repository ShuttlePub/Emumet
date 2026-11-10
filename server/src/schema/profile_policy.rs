//! Profile identity / relationship / domain-controls contract schemas (E8).
//!
//! **Contract only.** The routes that use these schemas are described in OpenAPI
//! but are NOT mounted on the production root router; storage and relationship
//! processing arrive in later units (P1, P2, P3o/P3i, D8P, D8G, D8R) and the
//! production mount happens in P4. All external IDs are nanoids.
//!
//! Nothing constructs or reads these types yet (they only feed OpenAPI generation), hence the
//! module-wide `dead_code` allowance, to be dropped once P1/P2/D8P implement the handlers.
#![allow(dead_code)]

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

/// Profile-scoped follow list (following or followers of a Profile).
#[derive(Debug, Serialize, ToSchema)]
pub struct ProfileFollowListResponse {
    pub items: Vec<ProfileFollowResponse>,
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
#[derive(Debug, Serialize, ToSchema)]
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
