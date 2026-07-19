use super::super::outbound_follow::find_existing_following;
use super::super::remote_actor::{resolve_remote_actor, upsert_remote_account};
use super::super::{local_actor_url, ACTIVITYSTREAMS_CONTEXT};
use super::InboxUseCase;
use crate::transfer::activitypub::InboxActivityDto;
use error_stack::{Report, ResultExt};
use kernel::activitypub::Activity;
use kernel::interfaces::config::PublicBaseUrl;
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::repository::{FollowRepository, RemoteAccountRepository};
use kernel::prelude::entity::{
    Follow, FollowApprovedAt, FollowId, FollowTargetId, OutboxActivity, OutboxActivityId,
    RemoteAccountUrl,
};
use kernel::KernelError;
use serde_json::Value;

pub(super) async fn handle_follow_activity<T>(
    module: &T,
    dto: InboxActivityDto,
) -> error_stack::Result<(), KernelError>
where
    T: InboxUseCase + ?Sized,
{
    let followed_actor_url = activity_object_id(&dto.activity).ok_or_else(|| {
        Report::new(KernelError::Rejected)
            .attach_printable("Follow activity object must be an actor id")
    })?;
    ensure_local_actor_matches(
        module.public_base_url(),
        &dto.account_nanoid,
        &followed_actor_url,
    )?;

    let remote_actor = resolve_remote_actor(&dto.activity.actor).await?;
    let mut executor = module.database_connection().get_executor().await?;
    let remote_account = upsert_remote_account(
        module.remote_account_repository(),
        &mut executor,
        remote_actor,
    )
    .await?;

    let source = FollowTargetId::from(remote_account.id().clone());
    let destination = FollowTargetId::from(dto.account_id.clone());
    let follow = match find_existing_follow(
        module.follow_repository(),
        &mut executor,
        &source,
        &destination,
    )
    .await?
    {
        Some(_existing) => {
            tracing::debug!("Follow already exists, skipping Accept creation");
            return Ok(());
        }
        None => {
            let follow = Follow::new(
                FollowId::new(kernel::generate_id()),
                source,
                destination,
                Some(FollowApprovedAt::default()),
            )?;
            module
                .follow_repository()
                .create(&mut executor, &follow)
                .await?;
            follow
        }
    };

    let local_actor_url = local_actor_url(module.public_base_url(), &dto.account_nanoid);
    let accept = accept_activity(
        module.public_base_url(),
        &follow,
        &local_actor_url,
        dto.activity.clone(),
    )?;
    if let Err(error) = module
        .deliver_accept(&dto.account_id, remote_account.inbox_url(), &accept)
        .await
    {
        tracing::warn!(?error, inbox_url = ?remote_account.inbox_url(), "Failed to deliver ActivityPub Accept");
    }

    let outbox_entry = OutboxActivity {
        id: OutboxActivityId::default(),
        account_id: dto.account_id.clone(),
        activity_id: accept.id.clone(),
        activity_type: "Accept".to_string(),
        object_json: serde_json::to_string(&accept).map_err(|e| {
            Report::new(KernelError::Internal)
                .attach_printable(format!("Failed to serialize Accept activity to JSON: {e}"))
        })?,
        created_at: time::OffsetDateTime::now_utc(),
    };
    module
        .store_outbox_activity(&outbox_entry)
        .await
        .change_context_lazy(|| KernelError::Internal)
        .attach_printable("Failed to store outbox activity")?;

    Ok(())
}

pub(super) async fn handle_undo_follow<T>(
    module: &T,
    dto: InboxActivityDto,
) -> error_stack::Result<(), KernelError>
where
    T: InboxUseCase + ?Sized,
{
    let follow_activity = undo_follow_object(&dto.activity).ok_or_else(|| {
        Report::new(KernelError::Rejected)
            .attach_printable("Undo activity object must be a Follow activity")
    })?;
    let followed_actor_url = activity_object_id(&follow_activity).ok_or_else(|| {
        Report::new(KernelError::Rejected)
            .attach_printable("Undo Follow object must target an actor id")
    })?;
    ensure_local_actor_matches(
        module.public_base_url(),
        &dto.account_nanoid,
        &followed_actor_url,
    )?;

    let mut executor = module.database_connection().get_executor().await?;
    let remote_url = RemoteAccountUrl::new(dto.activity.actor.clone());
    let Some(remote_account) = module
        .remote_account_repository()
        .find_by_url(&mut executor, &remote_url)
        .await?
    else {
        return Ok(());
    };

    let source = FollowTargetId::from(remote_account.id().clone());
    let destination = FollowTargetId::from(dto.account_id);
    if let Some(follow) = find_existing_follow(
        module.follow_repository(),
        &mut executor,
        &source,
        &destination,
    )
    .await?
    {
        module
            .follow_repository()
            .delete(&mut executor, follow.id())
            .await?;
    }
    Ok(())
}

pub(super) async fn handle_accept_activity<T>(
    module: &T,
    dto: InboxActivityDto,
) -> error_stack::Result<(), KernelError>
where
    T: InboxUseCase + ?Sized,
{
    let accept = &dto.activity;
    let nested_follow = accept
        .object
        .as_ref()
        .and_then(|obj| serde_json::from_value::<Activity>(obj.clone()).ok())
        .filter(|a| a.type_ == "Follow")
        .ok_or_else(|| {
            Report::new(KernelError::Rejected)
                .attach_printable("Accept object must be a Follow activity")
        })?;

    let follow_actor_url = nested_follow.actor.trim_end_matches('/').to_string();
    let expected_local = local_actor_url(module.public_base_url(), &dto.account_nanoid);
    if follow_actor_url != expected_local.trim_end_matches('/') {
        tracing::debug!(
            follow_actor = %nested_follow.actor,
            expected = %expected_local,
            "Accept Follow actor does not match local actor"
        );
        return Ok(());
    }

    let remote_actor_url = activity_object_id(&nested_follow).ok_or_else(|| {
        Report::new(KernelError::Rejected)
            .attach_printable("Accept Follow object must have an actor id")
    })?;

    let accept_actor = accept.actor.trim_end_matches('/').to_string();
    if accept_actor != remote_actor_url.trim_end_matches('/') {
        tracing::debug!(
            accept_actor = %accept.actor,
            remote_actor = %remote_actor_url,
            "Accept actor does not match Follow object"
        );
        return Ok(());
    }

    let mut executor = module.database_connection().get_executor().await?;
    let remote_url = RemoteAccountUrl::new(remote_actor_url.clone());
    let remote_account = module
        .remote_account_repository()
        .find_by_url(&mut executor, &remote_url)
        .await?
        .ok_or_else(|| {
            Report::new(KernelError::NotFound)
                .attach_printable(format!("Remote account not found for {remote_actor_url}"))
        })?;

    let source = FollowTargetId::from(dto.account_id.clone());
    let destination = FollowTargetId::from(remote_account.id().clone());
    if let Some(existing) = find_existing_following(
        module.follow_repository(),
        &mut executor,
        &source,
        &destination,
    )
    .await?
    {
        if existing.approved_at().is_none() {
            let approved = Follow::new(
                existing.id().clone(),
                existing.source().clone(),
                existing.destination().clone(),
                Some(FollowApprovedAt::default()),
            )?;
            module
                .follow_repository()
                .update(&mut executor, &approved)
                .await?;
            tracing::info!(
                remote_actor = %remote_actor_url,
                "Follow approved via Accept activity"
            );
        }
    } else {
        tracing::debug!(
            remote_actor = %remote_actor_url,
            "No pending follow found for Accept activity"
        );
    }
    Ok(())
}

pub(super) fn undo_object_is_follow(activity: &Activity) -> bool {
    undo_follow_object(activity).is_some()
}

fn undo_follow_object(activity: &Activity) -> Option<Activity> {
    let object = activity.object.as_ref()?;
    serde_json::from_value::<Activity>(object.clone())
        .ok()
        .filter(|activity| activity.type_ == "Follow")
}

fn activity_object_id(activity: &Activity) -> Option<String> {
    match activity.object.as_ref()? {
        Value::String(value) => Some(value.clone()),
        Value::Object(map) => map.get("id").and_then(Value::as_str).map(str::to_string),
        _ => None,
    }
}

fn ensure_local_actor_matches(
    public_base_url: &PublicBaseUrl,
    account_nanoid: &str,
    object_id: &str,
) -> error_stack::Result<(), KernelError> {
    let expected = local_actor_url(public_base_url, account_nanoid);
    if object_id.trim_end_matches('/') == expected {
        Ok(())
    } else {
        Err(Report::new(KernelError::Rejected).attach_printable(format!(
            "Follow object does not match local actor: expected {expected}, got {object_id}"
        )))
    }
}

async fn find_existing_follow<R, E>(
    repository: &R,
    executor: &mut E,
    source: &FollowTargetId,
    destination: &FollowTargetId,
) -> error_stack::Result<Option<Follow>, KernelError>
where
    R: FollowRepository<Executor = E>,
    E: kernel::interfaces::database::Executor,
{
    let followers = repository.find_followers(executor, destination).await?;
    Ok(followers
        .into_iter()
        .find(|follow| follow.source() == source && follow.destination() == destination))
}

fn accept_activity(
    public_base_url: &PublicBaseUrl,
    follow: &Follow,
    actor: &str,
    original_follow: Activity,
) -> error_stack::Result<Activity, KernelError> {
    // The Accept activity must be directed TO the follower (original Follow's actor),
    // not to the local actor who is sending the Accept.
    let remote_follower_url = &original_follow.actor;
    let object = serde_json::to_value(original_follow.clone()).map_err(|e| {
        Report::from(e)
            .change_context(KernelError::Internal)
            .attach_printable("Failed to serialize original Follow activity")
    })?;
    Ok(Activity {
        context: Some(Value::String(ACTIVITYSTREAMS_CONTEXT.to_string())),
        id: format!(
            "{}/activities/{}",
            public_base_url.as_str().trim_end_matches('/'),
            follow.id().as_ref()
        ),
        type_: "Accept".to_string(),
        actor: actor.to_string(),
        object: Some(object),
        target: None,
        to: Some(vec![remote_follower_url.to_string()]),
        cc: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::interfaces::config::DependOnPublicBaseUrl;
    use kernel::prelude::entity::AccountId;

    struct MockModule {
        public_base_url: PublicBaseUrl,
    }

    impl DependOnPublicBaseUrl for MockModule {
        fn public_base_url(&self) -> &PublicBaseUrl {
            &self.public_base_url
        }
    }

    fn follow(source: AccountId, destination: AccountId, approved: bool) -> Follow {
        kernel::ensure_generator_initialized();
        Follow::new(
            FollowId::new(kernel::generate_id()),
            FollowTargetId::from(source),
            FollowTargetId::from(destination),
            approved.then(FollowApprovedAt::default),
        )
        .unwrap()
    }

    fn module() -> (MockModule, AccountId) {
        kernel::ensure_generator_initialized();
        let account_id = AccountId::default();

        (
            MockModule {
                public_base_url: PublicBaseUrl::new("https://example.com/".to_string()),
            },
            account_id,
        )
    }

    fn follow_activity(actor: &str, object: &str) -> Activity {
        Activity {
            context: None,
            id: "https://remote.example/activities/follow-1".to_string(),
            type_: "Follow".to_string(),
            actor: actor.to_string(),
            object: Some(serde_json::Value::String(object.to_string())),
            target: None,
            to: None,
            cc: None,
        }
    }

    #[test]
    fn undo_object_is_follow_detects_nested_follow() {
        let follow = follow_activity(
            "https://remote.example/users/bob",
            "https://example.com/accounts/alice",
        );
        let undo = Activity {
            context: None,
            id: "https://remote.example/activities/undo-1".to_string(),
            type_: "Undo".to_string(),
            actor: "https://remote.example/users/bob".to_string(),
            object: Some(serde_json::to_value(follow).unwrap()),
            target: None,
            to: None,
            cc: None,
        };

        assert!(undo_object_is_follow(&undo));
    }

    #[test]
    fn accept_activity_wraps_original_follow() {
        let (module, account_id) = module();
        let follow = follow(AccountId::default(), account_id, true);
        let original = follow_activity(
            "https://remote.example/users/bob",
            "https://example.com/accounts/alice",
        );

        let accept = accept_activity(
            module.public_base_url(),
            &follow,
            "https://example.com/accounts/alice",
            original,
        )
        .unwrap();

        assert_eq!(accept.type_, "Accept");
        assert_eq!(accept.actor, "https://example.com/accounts/alice");
        assert_eq!(
            accept.id,
            format!("https://example.com/activities/{}", follow.id().as_ref())
        );
        assert_eq!(
            accept.object.as_ref().and_then(|value| value.get("type")),
            Some(&serde_json::Value::String("Follow".to_string()))
        );
        // Accept must be directed TO the follower (original Follow's actor),
        // not to the local actor who is sending the Accept.
        assert_eq!(
            accept.to,
            Some(vec!["https://remote.example/users/bob".to_string()]),
            "Accept.to should target the remote follower, not the local actor"
        );
    }

    #[test]
    fn local_actor_match_rejects_wrong_follow_object() {
        let public_base_url = PublicBaseUrl::new("https://example.com/".to_string());

        assert!(ensure_local_actor_matches(
            &public_base_url,
            "alice",
            "https://example.com/ap/accounts/alice/"
        )
        .is_ok());
        assert!(ensure_local_actor_matches(
            &public_base_url,
            "alice",
            "https://example.com/accounts/bob"
        )
        .is_err());
    }
}
