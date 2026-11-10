//! Profile inbox: Follow / Accept / Reject / Undo(Follow) reception (P3i).
//!
//! 1 Profile = 1 actor. The Profile inbox applies exactly these activities (the "received activity
//! table"); every other activity (post activities such as Create / Like / EmojiReact / Announce,
//! Block / Undo(Block), Undo of anything but a Follow, ...) is ignored without touching any
//! relation state — post authorization belongs to ShuttlePub. Domain policy (D8G) is applied to
//! **Follow only**: a domain `block` refuses new Follows and stale Accepts, `mute` never does, and
//! Undo / Reject (clean-up that ends a relation) are processed even while a block is active.
//!
//! | activity | effect |
//! |---|---|
//! | `Follow` (object = this Profile) | stores an accepted generation and reserves an `Accept` job, in one transaction. If this Profile blocks the signer individually, or blocks the **domain** of the signer's canonical actor ID (D8G): stores an ended(blocked) generation and reserves a `Reject(Follow)` instead |
//! | `Accept` (object = Follow sent by this Profile) | pending generation -> accepted. **Not applied while the signer's domain is blocked** (D8G): the generation stays as it is and the Accept is only recorded, so replaying it after an unblock changes nothing |
//! | `Reject` (object = Follow sent by this Profile) | active generation -> ended(rejected) |
//! | `Undo` (object = Follow of the signer) | active generation -> ended(undone); when the Follow never arrived: an embedded Follow aimed at this Profile is stored as an ended generation, a bare Follow ID only as an inbound record scoped to this receiver |
//!
//! Verification order (nothing is written before every check passed):
//! 1. the HTTP Signature was verified by the route; its key owner is the signer actor and must be
//!    the `activity.actor`;
//! 2. the inbox Profile must exist and be an actor (username + active key), else not found;
//! 3. `Follow` must target exactly this Profile; embedded Follow objects must name the signer as
//!    their actor (`Undo`) and this Profile as the target;
//! 4. `Accept` / `Reject` / `Undo` are matched to a stored generation by the **original Follow
//!    activity ID** (P2 generation identifier) and by both parties of that generation.
//!
//! Every transaction takes the Profile relationship lock first (`ProfileRelationshipLock`, the
//! convention shared with the sending side and block writes), so block checks, Follow storage and
//! Undo / Accept / Reject cannot interleave with a concurrent block write or with each other.
//!
//! Duplicates are detected with the P2 inbound activity record, written in the same transaction as
//! the relation change: a failure rolls the record back, so a redelivery is processed again.
//! Stale, foreign and out-of-order activities never create, revive or delete a relation other than
//! the one the activity legitimately refers to.

mod parse;

use super::remote_actor::resolve_remote_actor;
use super::ACTIVITYSTREAMS_CONTEXT;
use crate::dto::profile_inbound::{ProfileInboxDto, ProfileInboxIgnored, ProfileInboxOutcome};
use crate::service::domain_follow_guard::domain_blocks_actor;
use error_stack::Report;
use kernel::activitypub::{Activity, ActorUrlBuilder};
use kernel::interfaces::config::DependOnPublicBaseUrl;
use kernel::interfaces::database::{
    DatabaseConnection, DependOnDatabaseConnection, DependOnTransactionManager, TransactionManager,
};
use kernel::interfaces::read_model::{DependOnAccountQuery, DependOnProfileQuery, ProfileQuery};
use kernel::interfaces::repository::{
    BeginFollowOutcome, DependOnDomainControlRepository, DependOnInboundActivityRepository,
    DependOnProfileBlockRepository, DependOnProfileDeliveryEnqueuer,
    DependOnProfileFollowRepository, DependOnProfileRelationshipLock,
    DependOnProfileSigningKeyRepository, InboundActivityRepository, ProfileBlockRepository,
    ProfileDeliveryEnqueuer, ProfileFollowRepository, ProfileRelationshipLock,
};
use kernel::prelude::entity::{
    DeliveryActivityType, GraphActivityId, GraphActorUrl, GraphInboxUrl, Nanoid,
    NewInboundActivity, NewProfileDeliveryJob, Profile, ProfileFollow, ProfileFollowEndReason,
    ProfileFollowId, ProfileFollowParts, ProfileFollowState, ProfileGraphActor, ProfileId,
    ProfileSigningKeyRepository,
};
use kernel::KernelError;
use parse::{follow_object, reference_id, same_id, FollowObject};
use std::future::Future;
use time::OffsetDateTime;

pub trait ProfileInboxUseCase:
    'static
    + Sync
    + Send
    + Clone
    + DependOnProfileQuery
    + DependOnAccountQuery
    + DependOnProfileSigningKeyRepository
    + DependOnProfileFollowRepository
    + DependOnInboundActivityRepository
    + DependOnProfileDeliveryEnqueuer
    + DependOnProfileBlockRepository
    + DependOnDomainControlRepository
    + DependOnProfileRelationshipLock
    + DependOnTransactionManager
    + DependOnPublicBaseUrl
{
    fn handle_profile_inbox_activity(
        &self,
        dto: ProfileInboxDto,
    ) -> impl Future<Output = error_stack::Result<ProfileInboxOutcome, KernelError>> + Send {
        handle(self, dto)
    }
}

impl<T> ProfileInboxUseCase for T where
    T: 'static
        + Sync
        + Send
        + Clone
        + DependOnProfileQuery
        + DependOnAccountQuery
        + DependOnProfileSigningKeyRepository
        + DependOnProfileFollowRepository
        + DependOnInboundActivityRepository
        + DependOnProfileDeliveryEnqueuer
        + DependOnProfileBlockRepository
        + DependOnDomainControlRepository
        + DependOnProfileRelationshipLock
        + DependOnTransactionManager
        + DependOnPublicBaseUrl
{
}

fn rejected(message: &str) -> Report<KernelError> {
    Report::new(KernelError::Rejected).attach_printable(message.to_string())
}

/// What every handler needs after the common checks.
struct Context {
    receiver: ProfileId,
    local_actor_url: String,
    signer: GraphActorUrl,
}

struct Identified {
    context: Context,
    activity_id: GraphActivityId,
}

async fn handle<T>(
    module: &T,
    dto: ProfileInboxDto,
) -> error_stack::Result<ProfileInboxOutcome, KernelError>
where
    T: ProfileInboxUseCase,
{
    let ProfileInboxDto {
        profile_nanoid,
        signer_actor_url,
        activity,
    } = dto;
    // The route already compared the key owner with the activity actor; keep the invariant here
    // so that no caller can hand over an activity the signer did not send.
    if !same_id(&signer_actor_url, &activity.actor) {
        return Err(Report::new(KernelError::PermissionDenied)
            .attach_printable("HTTP Signature actor does not match activity.actor"));
    }
    let (receiver, local_actor_url) = resolve_receiver(module, &profile_nanoid).await?;

    let supported = matches!(
        activity.type_.as_str(),
        "Follow" | "Accept" | "Reject" | "Undo"
    );
    if !supported {
        return Ok(ProfileInboxOutcome::Ignored(
            ProfileInboxIgnored::UnsupportedActivity,
        ));
    }
    let base = module.public_base_url().as_str().trim_end_matches('/');
    if signer_actor_url.starts_with(&format!("{base}/ap/")) {
        return Err(rejected(
            "a local actor cannot deliver to a Profile inbox over HTTP",
        ));
    }
    let signer = GraphActorUrl::new(signer_actor_url)?;
    let context = Context {
        receiver,
        local_actor_url,
        signer,
    };
    match activity.type_.as_str() {
        "Follow" => {
            let identified = identify(context, &activity)?;
            handle_follow(module, identified, activity).await
        }
        "Accept" => handle_response(module, context, activity, Response::Accept).await,
        "Reject" => handle_response(module, context, activity, Response::Reject).await,
        _ => handle_undo(module, context, activity).await,
    }
}

fn identify(context: Context, activity: &Activity) -> error_stack::Result<Identified, KernelError> {
    Ok(Identified {
        context,
        activity_id: GraphActivityId::new(activity.id.clone())?,
    })
}

/// The inbox Profile must exist and be an actor (username and an active key), exactly like the
/// actor document: otherwise the inbox does not exist either.
async fn resolve_receiver<T>(
    module: &T,
    profile_nanoid: &str,
) -> error_stack::Result<(ProfileId, String), KernelError>
where
    T: ProfileInboxUseCase,
{
    let mut executor = module.database_connection().connection().await?;
    let profile = module
        .profile_query()
        .find_by_nanoid(
            &mut executor,
            &Nanoid::<Profile>::new(profile_nanoid.to_string()),
        )
        .await?
        .ok_or_else(|| {
            Report::new(KernelError::NotFound)
                .attach_printable(format!("Profile not found: {profile_nanoid}"))
        })?;
    let not_an_actor = |reason: &str| {
        Report::new(KernelError::NotFound).attach_printable(format!(
            "Profile {profile_nanoid} is not an actor: {reason}"
        ))
    };
    if profile.username().is_none() {
        return Err(not_an_actor("no username"));
    }
    // A suspended / banned / deactivated owner Account has no inbox (as the legacy Account actor).
    if !crate::service::profile_owner_status::owner_account_is_active(
        module,
        &mut executor,
        profile.account_id(),
    )
    .await?
    {
        return Err(not_an_actor("owner account is not active"));
    }
    if module
        .profile_signing_key_repository()
        .find_active_by_profile_id(&mut executor, profile.id())
        .await?
        .is_empty()
    {
        return Err(not_an_actor("no active signing key"));
    }
    let actor_url =
        ActorUrlBuilder::for_profile(module.public_base_url().as_str(), profile_nanoid).actor_id();
    Ok((profile.id().clone(), actor_url))
}

fn inbound_record(
    context: &Context,
    activity_id: &GraphActivityId,
    activity_type: &str,
    now: OffsetDateTime,
) -> error_stack::Result<NewInboundActivity, KernelError> {
    NewInboundActivity::new(
        context.receiver.clone(),
        context.signer.clone(),
        activity_id.clone(),
        activity_type,
        now,
    )
}

fn ignored(reason: ProfileInboxIgnored) -> ProfileInboxOutcome {
    ProfileInboxOutcome::Ignored(reason)
}

async fn resolve_inbox(signer: &GraphActorUrl) -> error_stack::Result<GraphInboxUrl, KernelError> {
    // A signed Follow cannot be answered without the follower's inbox. Failing here (before any
    // write) with a retryable status lets the sender redeliver instead of losing the Follow.
    let actor = resolve_remote_actor(signer.as_str()).await.map_err(|e| {
        e.change_context(KernelError::Timeout)
            .attach_printable("remote follower actor could not be resolved")
    })?;
    let inbox = actor
        .inbox_url()
        .ok_or_else(|| rejected("remote follower actor has no inbox"))?;
    GraphInboxUrl::new(inbox)
}

fn accept_payload(
    local_actor_url: &str,
    follow: &ProfileFollow,
    follow_activity: &Activity,
    follower: &GraphActorUrl,
) -> error_stack::Result<(GraphActivityId, String), KernelError> {
    let accept_id = GraphActivityId::new(format!(
        "{local_actor_url}#accepts/follows/{}",
        follow.nanoid().as_ref()
    ))?;
    let object = serde_json::to_value(follow_activity).map_err(|e| {
        Report::from(e)
            .change_context(KernelError::Internal)
            .attach_printable("Failed to serialize the received Follow activity")
    })?;
    let payload = serde_json::json!({
        "@context": ACTIVITYSTREAMS_CONTEXT,
        "id": accept_id.as_str(),
        "type": "Accept",
        "actor": local_actor_url,
        "object": object,
        "to": [follower.as_str()],
    });
    let payload = serde_json::to_string(&payload).map_err(|e| {
        Report::from(e)
            .change_context(KernelError::Internal)
            .attach_printable("Failed to serialize the Accept activity")
    })?;
    Ok((accept_id, payload))
}

async fn handle_follow<T>(
    module: &T,
    identified: Identified,
    activity: Activity,
) -> error_stack::Result<ProfileInboxOutcome, KernelError>
where
    T: ProfileInboxUseCase,
{
    let Identified {
        context,
        activity_id,
    } = identified;
    let target = activity
        .object
        .as_ref()
        .and_then(reference_id)
        .ok_or_else(|| rejected("Follow object must be an actor id"))?;
    if !same_id(&target, &context.local_actor_url) {
        return Err(rejected("Follow does not target the inbox Profile"));
    }

    let mut executor = module.database_connection().connection().await?;
    let already_received = module
        .inbound_activity_repository()
        .find(
            &mut executor,
            &context.receiver,
            &context.signer,
            &activity_id,
        )
        .await?
        .is_some();
    drop(executor);
    // Network access happens outside the transaction, and not at all for a known duplicate.
    let inbox = if already_received {
        None
    } else {
        Some(resolve_inbox(&context.signer).await?)
    };

    let deps = module.clone();
    module
        .transaction_manager()
        .transaction(move |executor| {
            Box::pin(async move {
                // Same lock convention as the sending side and block writes: every transaction that
                // decides on blocks or changes a Follow of this Profile takes the Profile lock first.
                deps.profile_relationship_lock()
                    .lock_profiles(executor, std::slice::from_ref(&context.receiver))
                    .await?;
                let now = OffsetDateTime::now_utc();
                let receipt = deps
                    .inbound_activity_repository()
                    .record(
                        executor,
                        &inbound_record(&context, &activity_id, "Follow", now)?,
                    )
                    .await?;
                if !receipt.first_seen {
                    return Ok(ProfileInboxOutcome::Duplicate);
                }
                let inbox = inbox.ok_or_else(|| {
                    Report::new(KernelError::Internal)
                        .attach_printable("first Follow reception without a resolved inbox")
                })?;
                // Block decisions (D8G). Two kinds of block refuse a new Follow, both decided under the
                // Profile lock taken above, so a concurrent block write either commits first (the
                // Follow is refused here) or waits for this transaction and then ends the relation
                // created below:
                //  * an individual block of the signer by this Profile (P3i);
                //  * a domain `block` of this Profile whose domain equals the host of the signer's
                //    verified canonical actor ID (exact match; the inbox / sharedInbox / CDN host is
                //    never consulted). A domain `mute` does not refuse a Follow (display
                //    suppression only).
                // The refusal is the same for both: an ended(blocked) generation (no active
                // relation, replays never revive it) and a `Reject(Follow)` job, never an AP Block.
                let signer_actor = ProfileGraphActor::Remote(context.signer.clone());
                let blocked_individually = deps
                    .profile_block_repository()
                    .is_blocked(executor, &context.receiver, &signer_actor)
                    .await?;
                let blocked_by_domain = !blocked_individually
                    && domain_blocks_actor(
                        &deps,
                        executor,
                        &context.receiver,
                        context.signer.as_str(),
                    )
                    .await?;
                if blocked_individually || blocked_by_domain {
                    return refuse_blocked_follow(
                        &deps,
                        executor,
                        &context,
                        &activity_id,
                        inbox,
                        blocked_by_domain,
                        now,
                    )
                    .await;
                }
                let follow = ProfileFollow::new_accepted(
                    ProfileFollowId::new(kernel::generate_id()),
                    ProfileGraphActor::Remote(context.signer.clone()),
                    ProfileGraphActor::Local(context.receiver.clone()),
                    activity_id.clone(),
                    now,
                )?;
                let mut replaced_previous = false;
                loop {
                    match deps
                        .profile_follow_repository()
                        .begin_generation(executor, &follow)
                        .await?
                    {
                        BeginFollowOutcome::Created => break,
                        BeginFollowOutcome::ReplayedActivity(_) => {
                            return Ok(ignored(ProfileInboxIgnored::AlreadyApplied));
                        }
                        BeginFollowOutcome::ActiveGenerationExists(existing) => {
                            // The signer itself sent a new Follow while an older generation is
                            // still active (it lost its state or never saw our Accept). Only the
                            // actor's own generation is replaced, never anyone else's.
                            if replaced_previous {
                                return Err(Report::new(KernelError::Concurrency)
                                    .attach_printable("follow generation changed concurrently"));
                            }
                            deps.profile_follow_repository()
                                .end(executor, existing.id(), ProfileFollowEndReason::Undone, now)
                                .await?;
                            replaced_previous = true;
                        }
                    }
                }
                let (accept_id, payload) = accept_payload(
                    &context.local_actor_url,
                    &follow,
                    &activity,
                    &context.signer,
                )?;
                let job = NewProfileDeliveryJob::new(
                    context.receiver.clone(),
                    DeliveryActivityType::Accept,
                    accept_id,
                    inbox,
                    payload,
                    now,
                )?
                .with_follow(follow.id().clone());
                deps.profile_delivery_enqueuer()
                    .enqueue(executor, &job)
                    .await?;
                Ok(ProfileInboxOutcome::FollowAccepted { replaced_previous })
            })
        })
        .await
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Response {
    Accept,
    Reject,
}

impl Response {
    fn activity_type(self) -> &'static str {
        match self {
            Self::Accept => "Accept",
            Self::Reject => "Reject",
        }
    }
}

/// Accept / Reject of a Follow this Profile sent: the Profile is the follower and the signer must
/// be the followee of the referenced generation.
async fn handle_response<T>(
    module: &T,
    context: Context,
    activity: Activity,
    response: Response,
) -> error_stack::Result<ProfileInboxOutcome, KernelError>
where
    T: ProfileInboxUseCase,
{
    let follow_id = match follow_object(activity.object.as_ref()) {
        FollowObject::Missing => return Err(rejected("object must be a Follow or its id")),
        FollowObject::Other => return Ok(ignored(ProfileInboxIgnored::UnsupportedActivity)),
        FollowObject::Id(id) => id,
        FollowObject::Embedded { id, actor, object } => {
            // An embedded Follow that is not ours, or that was not aimed at the signer, refers
            // to a relation this activity cannot decide.
            let ours = actor
                .as_deref()
                .is_some_and(|actor| same_id(actor, &context.local_actor_url));
            let aimed_at_signer = object
                .as_deref()
                .is_none_or(|object| same_id(object, context.signer.as_str()));
            if !ours || !aimed_at_signer {
                return Ok(ignored(ProfileInboxIgnored::ForeignFollow));
            }
            id.ok_or_else(|| rejected("embedded Follow has no id"))?
        }
    };
    let activity_id = GraphActivityId::new(activity.id.clone())?;
    let Ok(follow_activity_id) = GraphActivityId::new(follow_id) else {
        return Ok(ignored(ProfileInboxIgnored::UnknownFollow));
    };

    let deps = module.clone();
    module
        .transaction_manager()
        .transaction(move |executor| {
            Box::pin(async move {
                // Same lock convention as the sending side and block writes: every transaction that
                // decides on blocks or changes a Follow of this Profile takes the Profile lock first.
                deps.profile_relationship_lock()
                    .lock_profiles(executor, std::slice::from_ref(&context.receiver))
                    .await?;
                let now = OffsetDateTime::now_utc();
                let receipt = deps
                    .inbound_activity_repository()
                    .record(
                        executor,
                        &inbound_record(&context, &activity_id, response.activity_type(), now)?,
                    )
                    .await?;
                if !receipt.first_seen {
                    return Ok(ProfileInboxOutcome::Duplicate);
                }
                let follows = deps.profile_follow_repository();
                let Some(generation) = follows
                    .find_by_follower_activity(
                        executor,
                        &ProfileGraphActor::Local(context.receiver.clone()),
                        &follow_activity_id,
                    )
                    .await?
                else {
                    return Ok(ignored(ProfileInboxIgnored::UnknownFollow));
                };
                if generation.followee() != &ProfileGraphActor::Remote(context.signer.clone()) {
                    return Ok(ignored(ProfileInboxIgnored::ForeignFollow));
                }
                match (response, generation.state()) {
                    (_, ProfileFollowState::Ended) => Ok(ignored(ProfileInboxIgnored::StaleFollow)),
                    (Response::Accept, ProfileFollowState::Accepted) => {
                        Ok(ignored(ProfileInboxIgnored::AlreadyApplied))
                    }
                    (Response::Accept, ProfileFollowState::Pending) => {
                        // D8G: an Accept from a blocked domain never turns a Follow into a
                        // relation (it may have been sent before the block and arrive late). The
                        // reception stays recorded above, so replaying it after an unblock is a
                        // duplicate and cannot accept the Follow either.
                        if domain_blocks_actor(
                            &deps,
                            executor,
                            &context.receiver,
                            context.signer.as_str(),
                        )
                        .await?
                        {
                            return Ok(ignored(ProfileInboxIgnored::DomainBlocked));
                        }
                        if follows.accept(executor, generation.id(), now).await? {
                            Ok(ProfileInboxOutcome::FollowAcceptedByRemote)
                        } else {
                            Ok(ignored(ProfileInboxIgnored::AlreadyApplied))
                        }
                    }
                    (Response::Reject, _) => {
                        if follows
                            .end(
                                executor,
                                generation.id(),
                                ProfileFollowEndReason::Rejected,
                                now,
                            )
                            .await?
                        {
                            Ok(ProfileInboxOutcome::FollowRejectedByRemote)
                        } else {
                            Ok(ignored(ProfileInboxIgnored::AlreadyApplied))
                        }
                    }
                }
            })
        })
        .await
}

type Conn<T> =
    <<T as DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Connection;

/// A Follow refused because this Profile blocks the signer. The refusal is stored as an ended
/// (blocked) generation under the Follow's activity ID, so a replay of the same Follow (also
/// after the block is removed) can never create a relation, and a `Reject(Follow)` is reserved in
/// the same transaction ("a Follow of the other side is answered with Reject", never an AP Block).
async fn refuse_blocked_follow<T>(
    deps: &T,
    executor: &mut Conn<T>,
    context: &Context,
    activity_id: &GraphActivityId,
    inbox: GraphInboxUrl,
    by_domain: bool,
    now: OffsetDateTime,
) -> error_stack::Result<ProfileInboxOutcome, KernelError>
where
    T: ProfileInboxUseCase,
{
    let refused = ProfileFollow::from_parts(ProfileFollowParts {
        id: ProfileFollowId::new(kernel::generate_id()),
        nanoid: Nanoid::default(),
        follower: ProfileGraphActor::Remote(context.signer.clone()),
        followee: ProfileGraphActor::Local(context.receiver.clone()),
        activity_id: activity_id.clone(),
        state: ProfileFollowState::Ended,
        end_reason: Some(ProfileFollowEndReason::Blocked),
        accepted_at: None,
        ended_at: Some(now),
        created_at: now,
        updated_at: now,
    })?;
    match deps
        .profile_follow_repository()
        .begin_generation(executor, &refused)
        .await?
    {
        BeginFollowOutcome::Created => {}
        _ => return Ok(ignored(ProfileInboxIgnored::AlreadyApplied)),
    }
    let reject_id = GraphActivityId::new(format!(
        "{}/activities/{}/reject",
        context.local_actor_url,
        refused.id().as_ref()
    ))?;
    let payload = serde_json::json!({
        "@context": ACTIVITYSTREAMS_CONTEXT,
        "id": reject_id.as_str(),
        "type": "Reject",
        "actor": context.local_actor_url,
        "object": {
            "id": activity_id.as_str(),
            "type": "Follow",
            "actor": context.signer.as_str(),
            "object": context.local_actor_url,
        },
        "to": [context.signer.as_str()],
    });
    let payload = serde_json::to_string(&payload).map_err(|e| {
        Report::from(e)
            .change_context(KernelError::Internal)
            .attach_printable("Failed to serialize the Reject activity")
    })?;
    let job = NewProfileDeliveryJob::new(
        context.receiver.clone(),
        DeliveryActivityType::Reject,
        reject_id,
        inbox,
        payload,
        now,
    )?
    .with_follow(refused.id().clone());
    deps.profile_delivery_enqueuer()
        .enqueue(executor, &job)
        .await?;
    Ok(if by_domain {
        ProfileInboxOutcome::FollowRefusedByDomainBlock
    } else {
        ProfileInboxOutcome::FollowRefusedByBlock
    })
}

/// Ends `generation` for an Undo of the signer, after re-checking both parties.
async fn undo_generation<T>(
    deps: &T,
    executor: &mut Conn<T>,
    context: &Context,
    generation: &ProfileFollow,
    now: OffsetDateTime,
) -> error_stack::Result<ProfileInboxOutcome, KernelError>
where
    T: ProfileInboxUseCase,
{
    if generation.follower() != &ProfileGraphActor::Remote(context.signer.clone())
        || generation.followee() != &ProfileGraphActor::Local(context.receiver.clone())
    {
        return Ok(ignored(ProfileInboxIgnored::ForeignFollow));
    }
    if !generation.is_active() {
        return Ok(ignored(ProfileInboxIgnored::AlreadyApplied));
    }
    if deps
        .profile_follow_repository()
        .end(
            executor,
            generation.id(),
            ProfileFollowEndReason::Undone,
            now,
        )
        .await?
    {
        Ok(ProfileInboxOutcome::FollowUndone)
    } else {
        Ok(ignored(ProfileInboxIgnored::AlreadyApplied))
    }
}

/// Undo of a Follow the signer sent to this Profile.
async fn handle_undo<T>(
    module: &T,
    context: Context,
    activity: Activity,
) -> error_stack::Result<ProfileInboxOutcome, KernelError>
where
    T: ProfileInboxUseCase,
{
    // `verified_target` = the Undo embeds the Follow and that Follow targets this Profile. A bare
    // Follow ID does not prove which Profile the Follow was aimed at.
    let (follow_id, verified_target) = match follow_object(activity.object.as_ref()) {
        FollowObject::Missing => return Err(rejected("Undo object must be a Follow or its id")),
        // Undo(Block) and every other Undo is outside the Profile inbox table.
        FollowObject::Other => return Ok(ignored(ProfileInboxIgnored::UnsupportedActivity)),
        FollowObject::Id(id) => (id, false),
        FollowObject::Embedded { id, actor, object } => {
            // Undoing somebody else's Follow, or a Follow aimed at another Profile, is a
            // protocol violation by the signer: refuse it as a whole.
            if !actor
                .as_deref()
                .is_some_and(|actor| same_id(actor, context.signer.as_str()))
            {
                return Err(rejected("embedded Follow was not sent by the Undo actor"));
            }
            if !object
                .as_deref()
                .is_some_and(|object| same_id(object, &context.local_actor_url))
            {
                return Err(rejected(
                    "embedded Follow does not target the inbox Profile",
                ));
            }
            (
                id.ok_or_else(|| rejected("embedded Follow has no id"))?,
                true,
            )
        }
    };
    let activity_id = GraphActivityId::new(activity.id.clone())?;
    let follow_activity_id = GraphActivityId::new(follow_id)?;

    let deps = module.clone();
    module
        .transaction_manager()
        .transaction(move |executor| {
            Box::pin(async move {
                deps.profile_relationship_lock()
                    .lock_profiles(executor, std::slice::from_ref(&context.receiver))
                    .await?;
                let now = OffsetDateTime::now_utc();
                let receipt = deps
                    .inbound_activity_repository()
                    .record(
                        executor,
                        &inbound_record(&context, &activity_id, "Undo", now)?,
                    )
                    .await?;
                if !receipt.first_seen {
                    return Ok(ProfileInboxOutcome::Duplicate);
                }
                let follows = deps.profile_follow_repository();
                let follower = ProfileGraphActor::Remote(context.signer.clone());
                let followee = ProfileGraphActor::Local(context.receiver.clone());
                if let Some(generation) = follows
                    .find_by_follower_activity(executor, &follower, &follow_activity_id)
                    .await?
                {
                    return undo_generation(&deps, executor, &context, &generation, now).await;
                }
                if !verified_target {
                    // The Undo overtook a Follow whose target Profile is unknown. A generation
                    // would be keyed by (follower, Follow ID) for every Profile, so it could
                    // swallow the genuine Follow addressed to another Profile. Remember the
                    // cancellation for this receiver only, as an inbound record under the
                    // Follow's ID: the same Follow delivered to this Profile later is a
                    // duplicate, a Follow delivered to any other Profile is unaffected.
                    let remembered = deps
                        .inbound_activity_repository()
                        .record(
                            executor,
                            &inbound_record(&context, &follow_activity_id, "UndoFollow", now)?,
                        )
                        .await?;
                    return Ok(if remembered.first_seen {
                        ProfileInboxOutcome::UndoRecordedBeforeFollow
                    } else {
                        ignored(ProfileInboxIgnored::AlreadyApplied)
                    });
                }
                // The Undo overtook its Follow and the target is proven to be this Profile:
                // remember the generation as ended so the late Follow (same activity ID) is a
                // replay and creates no relation.
                let tombstone = ProfileFollow::from_parts(ProfileFollowParts {
                    id: ProfileFollowId::new(kernel::generate_id()),
                    nanoid: Nanoid::default(),
                    follower,
                    followee,
                    activity_id: follow_activity_id,
                    state: ProfileFollowState::Ended,
                    end_reason: Some(ProfileFollowEndReason::Undone),
                    accepted_at: None,
                    ended_at: Some(now),
                    created_at: now,
                    updated_at: now,
                })?;
                match follows.begin_generation(executor, &tombstone).await? {
                    BeginFollowOutcome::Created => {
                        Ok(ProfileInboxOutcome::UndoRecordedBeforeFollow)
                    }
                    // The Follow was stored between the lookup and the insert: it is the
                    // generation this Undo is about, so end it (after re-checking the parties).
                    BeginFollowOutcome::ReplayedActivity(existing) => {
                        undo_generation(&deps, executor, &context, &existing, now).await
                    }
                    BeginFollowOutcome::ActiveGenerationExists(_) => {
                        Ok(ignored(ProfileInboxIgnored::AlreadyApplied))
                    }
                }
            })
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn follow_activity() -> Activity {
        serde_json::from_value(serde_json::json!({
            "@context": "https://www.w3.org/ns/activitystreams",
            "id": "https://remote.example/follows/1",
            "type": "Follow",
            "actor": "https://remote.example/users/bob",
            "object": "https://local.example/ap/profiles/p1",
        }))
        .unwrap()
    }

    #[test]
    fn accept_payload_wraps_the_received_follow_and_is_addressed_to_the_follower() {
        let follower = GraphActorUrl::new("https://remote.example/users/bob").unwrap();
        let follow = ProfileFollow::new_accepted(
            ProfileFollowId::new(1),
            ProfileGraphActor::Remote(follower.clone()),
            ProfileGraphActor::Local(ProfileId::new(2)),
            GraphActivityId::new("https://remote.example/follows/1").unwrap(),
            OffsetDateTime::now_utc(),
        )
        .unwrap();
        let (accept_id, payload) = accept_payload(
            "https://local.example/ap/profiles/p1",
            &follow,
            &follow_activity(),
            &follower,
        )
        .unwrap();
        assert_eq!(
            accept_id.as_str(),
            format!(
                "https://local.example/ap/profiles/p1#accepts/follows/{}",
                follow.nanoid().as_ref()
            )
        );
        let payload: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(payload["type"], "Accept");
        assert_eq!(payload["id"], accept_id.as_str());
        assert_eq!(payload["actor"], "https://local.example/ap/profiles/p1");
        assert_eq!(payload["to"][0], "https://remote.example/users/bob");
        assert_eq!(payload["object"]["id"], "https://remote.example/follows/1");
        assert_eq!(payload["object"]["type"], "Follow");
    }

    #[test]
    fn accept_id_is_stable_per_generation() {
        let follower = GraphActorUrl::new("https://remote.example/users/bob").unwrap();
        let follow = ProfileFollow::new_accepted(
            ProfileFollowId::new(1),
            ProfileGraphActor::Remote(follower.clone()),
            ProfileGraphActor::Local(ProfileId::new(2)),
            GraphActivityId::new("https://remote.example/follows/1").unwrap(),
            OffsetDateTime::now_utc(),
        )
        .unwrap();
        let build = || {
            accept_payload(
                "https://local.example/ap/profiles/p1",
                &follow,
                &follow_activity(),
                &follower,
            )
            .unwrap()
            .0
        };
        assert_eq!(build(), build());
    }
}
