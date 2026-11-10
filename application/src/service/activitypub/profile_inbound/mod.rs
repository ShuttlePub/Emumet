//! Profile inbox: Follow / Accept / Reject / Undo(Follow) reception (P3i).
//!
//! 1 Profile = 1 actor. The Profile inbox applies exactly these activities (the "received activity
//! table"); every other activity (post activities such as Create / Like / EmojiReact / Announce,
//! Block / Undo(Block), Undo of anything but a Follow, ...) is ignored without touching any
//! relation state — post authorization belongs to ShuttlePub, domain policy to D8G.
//!
//! | activity | effect |
//! |---|---|
//! | `Follow` (object = this Profile) | stores an accepted generation and reserves an `Accept` job, in one transaction |
//! | `Accept` (object = Follow sent by this Profile) | pending generation -> accepted |
//! | `Reject` (object = Follow sent by this Profile) | active generation -> ended(rejected) |
//! | `Undo` (object = Follow of the signer) | active generation -> ended(undone); stored as an ended generation when the Follow never arrived |
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
//! Duplicates are detected with the P2 inbound activity record, written in the same transaction as
//! the relation change: a failure rolls the record back, so a redelivery is processed again.
//! Stale, foreign and out-of-order activities never create, revive or delete a relation other than
//! the one the activity legitimately refers to.

mod parse;

use super::remote_actor::resolve_remote_actor;
use super::ACTIVITYSTREAMS_CONTEXT;
use crate::dto::profile_inbound::{ProfileInboxDto, ProfileInboxIgnored, ProfileInboxOutcome};
use error_stack::Report;
use kernel::activitypub::{Activity, ActorUrlBuilder};
use kernel::interfaces::config::DependOnPublicBaseUrl;
use kernel::interfaces::database::{
    DatabaseConnection, DependOnTransactionManager, TransactionManager,
};
use kernel::interfaces::read_model::{DependOnProfileQuery, ProfileQuery};
use kernel::interfaces::repository::{
    BeginFollowOutcome, DependOnInboundActivityRepository, DependOnProfileDeliveryEnqueuer,
    DependOnProfileFollowRepository, DependOnProfileSigningKeyRepository,
    InboundActivityRepository, ProfileDeliveryEnqueuer, ProfileFollowRepository,
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
    + DependOnProfileSigningKeyRepository
    + DependOnProfileFollowRepository
    + DependOnInboundActivityRepository
    + DependOnProfileDeliveryEnqueuer
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
        + DependOnProfileSigningKeyRepository
        + DependOnProfileFollowRepository
        + DependOnInboundActivityRepository
        + DependOnProfileDeliveryEnqueuer
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

    // Domain policy (D8G) plugs in here: a Follow refused by a block must be decided before the
    // generation is stored. P3i accepts every well-formed Follow.

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

/// Undo of a Follow the signer sent to this Profile.
async fn handle_undo<T>(
    module: &T,
    context: Context,
    activity: Activity,
) -> error_stack::Result<ProfileInboxOutcome, KernelError>
where
    T: ProfileInboxUseCase,
{
    let follow_id = match follow_object(activity.object.as_ref()) {
        FollowObject::Missing => return Err(rejected("Undo object must be a Follow or its id")),
        // Undo(Block) and every other Undo is outside the Profile inbox table.
        FollowObject::Other => return Ok(ignored(ProfileInboxIgnored::UnsupportedActivity)),
        FollowObject::Id(id) => id,
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
            id.ok_or_else(|| rejected("embedded Follow has no id"))?
        }
    };
    let activity_id = GraphActivityId::new(activity.id.clone())?;
    let follow_activity_id = GraphActivityId::new(follow_id)?;

    let deps = module.clone();
    module
        .transaction_manager()
        .transaction(move |executor| {
            Box::pin(async move {
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
                match follows
                    .find_by_follower_activity(executor, &follower, &follow_activity_id)
                    .await?
                {
                    Some(generation) if generation.followee() != &followee => {
                        Ok(ignored(ProfileInboxIgnored::ForeignFollow))
                    }
                    Some(generation) if !generation.is_active() => {
                        Ok(ignored(ProfileInboxIgnored::AlreadyApplied))
                    }
                    Some(generation) => {
                        if follows
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
                    None => {
                        // The Undo overtook its Follow. Remember the generation as ended so the
                        // late Follow (same activity ID) is a replay and creates no relation.
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
                            _ => Ok(ignored(ProfileInboxIgnored::AlreadyApplied)),
                        }
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
