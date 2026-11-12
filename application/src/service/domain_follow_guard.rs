//! Domain `block` applied to **Follow** (D8G): the shared decision helper and the lock convention.
//!
//! What is enforced (Follow only; posts are ShuttlePub's; bulk removal of existing relations is
//! D8R's, see `profile_relationship::domain_block`):
//!
//! | place | decision |
//! |---|---|
//! | inbound `Follow` (`profile_inbound`) | the Profile blocks the host of the signer's verified canonical actor ID -> refused like an individual block: ended(blocked) generation + `Reject(Follow)` job, no active relation, no `Accept` job |
//! | inbound `Accept` (`profile_inbound`) | the signer's host is blocked -> the pending Follow is **not** turned into a relation; the Accept is recorded (a replay after an unblock is a duplicate) |
//! | outbound `follow_profile` (`profile_relationship`) | the follower blocks the target's host (remote or local actor URL), or the local target blocks the follower's host -> refused (422) before any generation or job exists |
//! | delivery worker (`delivery`) | an unsent `Follow` / `Accept` job whose recipient host is blocked by the sending Profile is cancelled instead of sent, and the generation it belongs to ends as blocked |
//!
//! `Undo` / `Reject` (and the `Undo(Follow)` / `Reject(Follow)` jobs that end a relation) are never
//! refused by a domain block: clean-up is allowed while a block is active. `mute` never refuses a
//! Follow. The match is the D8P one: exact host of a **verified canonical actor ID**
//! (`ControlDomain::from_actor_id`), never an inbox / sharedInbox / CDN URL, never a parent domain.
//!
//! # Lock convention (shared with P3i / P3o, extended to domain controls)
//!
//! Every transaction that **decides on a block** or **changes a Follow** of a local Profile first
//! takes `ProfileRelationshipLock::lock_profiles` for every local Profile involved (ascending
//! `ProfileId`, once, before any relation read or write), and holds it until it ends:
//!
//! * inbound `Follow` / `Accept` / `Reject` / `Undo`: the receiving Profile;
//! * outbound `follow` / `block` / `unfollow`: the sender, plus the target when it is a local Profile for
//!   `follow` / `block` (a local -> local Follow therefore locks both Profiles in ascending order, so
//!   A -> B and B -> A at the same time cannot deadlock). `unfollow` takes the sender's lock because it
//!   updates the Follow row and then its delivery job, the reverse of the pre-send guard (job, then
//!   Follow): without the shared lock those row locks form a cycle and PostgreSQL aborts one side;
//! * delivery pre-send decision: the sending Profile (Profile row `FOR SHARE` first, then the lock);
//! * **domain control writes** (`put_domain_control` / `delete_domain_control`, and D8R's block
//!   application): the owning Profile, taken right after the owner check ([`lock_profile_for_domain_control_write`]).
//!
//! The decision reads ([`domain_blocks_actor`]) run on the same connection, after the lock, so
//! they see every block write that committed before the lock was granted, and a block write that
//! has not committed yet waits for the Follow transaction. A Follow that is stored under the lock
//! is therefore either refused by the block or ended by the block writer's relation removal
//! (D8R); the two never interleave into "block + active Follow".
//!
//! Lock order across resources: Profile row (`FOR SHARE`) -> relationship advisory lock(s) ->
//! Follow / job rows. Deleting a Profile (account projection, `DELETE FROM profiles`) takes the Profile
//! row first and cascades to Follow and job rows, so every transaction that updates those rows must
//! reach them through a Profile row lock: owner-checked paths (`follow`, `unfollow`, `block`, domain
//! control writes) hold `FOR SHARE` from the owner check, inbound transactions hold `FOR KEY SHARE`
//! from the first foreign-key insert (the inbound activity record), and the delivery pre-send guard
//! takes `ProfileRelationshipLock::lock_profile_row_shared` explicitly (it updates job -> Follow and
//! has no owner check; without it the cascade and the guard deadlock). `unblock`, `mute` and `unmute`
//! touch neither Follow nor delivery job rows (one block / mute row each; Follow decisions only read
//! them under the lock), so they do not take the advisory lock.
//!
//! D8R note: a domain block only needs the owning Profile's lock because every Follow transaction
//! involving that Profile (local or remote counterpart) takes it. When D8R also ends relations with
//! *local* counterparts (blocking this instance's own host), it must lock those Profiles in the
//! same single ascending `lock_profiles` call, so determine them before locking.

use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::repository::{
    DependOnDomainControlRepository, DependOnProfileRelationshipLock, DomainControlRepository,
    ProfileRelationshipLock,
};
use kernel::prelude::entity::{ControlDomain, DomainControlMode, ProfileId};
use kernel::KernelError;

type Conn<T> =
    <<T as DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Connection;

/// Whether `profile_id` blocks (mode `block`, exact host match) the actor with the given
/// **verified canonical actor ID**. `mute`, a different (sub / parent) domain, and an actor ID
/// without a DNS host never match.
///
/// Must run on the transaction connection of a transaction that already holds the relationship
/// lock of `profile_id`; see the module documentation.
pub async fn domain_blocks_actor<T>(
    deps: &T,
    executor: &mut Conn<T>,
    profile_id: &ProfileId,
    verified_actor_id: &str,
) -> error_stack::Result<bool, KernelError>
where
    T: DependOnDomainControlRepository,
{
    let Ok(domain) = ControlDomain::from_actor_id(verified_actor_id) else {
        return Ok(false);
    };
    Ok(deps
        .domain_control_repository()
        .find(executor, profile_id, &domain)
        .await?
        .is_some_and(|control| {
            control.mode() == DomainControlMode::Block
                && control.applies_to_actor_id(verified_actor_id)
        }))
}

/// Write side of the lock convention: take the owning Profile's relationship lock in the
/// transaction that creates / replaces / deletes a domain control. D8R's block write goes through
/// `profile_relationship::domain_block::lock_for_block_write`, which calls this for a plain write and
/// takes the same lock together with the local counterparts for a block.
pub async fn lock_profile_for_domain_control_write<T>(
    deps: &T,
    executor: &mut Conn<T>,
    profile_id: &ProfileId,
) -> error_stack::Result<(), KernelError>
where
    T: DependOnProfileRelationshipLock,
{
    deps.profile_relationship_lock()
        .lock_profiles(executor, std::slice::from_ref(profile_id))
        .await
}
