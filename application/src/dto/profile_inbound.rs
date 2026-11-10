//! DTOs of the Profile inbox (P3i) and of the followers / following reads.

use kernel::activitypub::Activity;
use kernel::prelude::entity::{
    GraphActorUrl, ProfileFollow, ProfileFollowState, ProfileGraphActor,
};
use time::OffsetDateTime;

/// One signed activity delivered to a Profile inbox.
#[derive(Debug, Clone)]
pub struct ProfileInboxDto {
    /// Nanoid of the Profile whose inbox received the request (from the URL path).
    pub profile_nanoid: String,
    /// The actor that owns the key which verified the HTTP Signature. Produced by the route after
    /// signature verification; the use case still requires it to equal `activity.actor`.
    pub signer_actor_url: String,
    pub activity: Activity,
}

/// Why a well-formed activity changed nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileInboxIgnored {
    /// Not part of the Profile inbox activity table (post activities, Block, Undo of a non-Follow ...).
    UnsupportedActivity,
    /// Accept / Reject refers to a Follow this Profile never sent.
    UnknownFollow,
    /// The referenced Follow generation belongs to another Profile or another actor.
    ForeignFollow,
    /// The referenced Follow generation already ended (an old Accept / Reject after Undo, ...).
    StaleFollow,
    /// The transition already happened or the generation is already known.
    AlreadyApplied,
}

/// What a processed Profile inbox activity did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileInboxOutcome {
    /// A remote Follow was stored as an accepted generation and an Accept was reserved in the same
    /// transaction. `replaced_previous` is true when an older active generation of the same actor
    /// was ended because the actor sent a new Follow.
    FollowAccepted {
        replaced_previous: bool,
    },
    /// The remote followee accepted this Profile's pending Follow.
    FollowAcceptedByRemote,
    /// The remote followee rejected (or removed) this Profile's Follow.
    FollowRejectedByRemote,
    /// The remote follower undid its Follow.
    FollowUndone,
    /// Undo(Follow) arrived before the Follow: the generation was stored as ended so the late
    /// Follow cannot create a relation.
    UndoRecordedBeforeFollow,
    /// The Profile blocks the follower: the Follow was refused (stored as an ended generation) and a
    /// `Reject(Follow)` was reserved in the same transaction. Nothing is accepted.
    FollowRefusedByBlock,
    /// Already received (P2 inbound activity record): nothing was processed again.
    Duplicate,
    Ignored(ProfileInboxIgnored),
}

/// A local or remote party of a follow relation as returned by the followers / following reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileActorRefDto {
    Local {
        actor_url: String,
        profile_nanoid: String,
    },
    Remote {
        actor_url: GraphActorUrl,
    },
}

impl ProfileActorRefDto {
    pub fn actor_url(&self) -> &str {
        match self {
            Self::Local { actor_url, .. } => actor_url,
            Self::Remote { actor_url } => actor_url.as_str(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileFollowStateDto {
    Pending,
    Accepted,
}

impl ProfileFollowStateDto {
    /// Only active generations are listed, so `Ended` never reaches a response.
    pub fn from_active(state: &ProfileFollowState) -> Option<Self> {
        match state {
            ProfileFollowState::Pending => Some(Self::Pending),
            ProfileFollowState::Accepted => Some(Self::Accepted),
            ProfileFollowState::Ended => None,
        }
    }
}

/// One row of the followers or following list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileFollowItemDto {
    /// Follow generation nanoid.
    pub id: String,
    /// The Profile that owns the list.
    pub profile_nanoid: String,
    /// The other party: the follower in the followers list, the target in the following list.
    pub counterpart: ProfileActorRefDto,
    pub state: ProfileFollowStateDto,
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileFollowPageDto {
    pub items: Vec<ProfileFollowItemDto>,
    pub next_cursor: Option<String>,
}

/// Which side of the relation the listed Profile is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileFollowDirection {
    /// The Profile is the follower.
    Following,
    /// The Profile is the followee.
    Followers,
}

impl ProfileFollowDirection {
    /// The party other than the listed Profile.
    pub fn counterpart<'a>(&self, follow: &'a ProfileFollow) -> &'a ProfileGraphActor {
        match self {
            Self::Following => follow.followee(),
            Self::Followers => follow.follower(),
        }
    }
}
