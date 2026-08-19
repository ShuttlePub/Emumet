use super::delivery::deliver_activity_to_inbox;
use super::{GetActorUseCase, ACTIVITYSTREAMS_CONTEXT};
use crate::dto::activitypub::GetActorDto;
use error_stack::Report;
use kernel::activitypub::{Activity, Actor};
use kernel::interfaces::config::{DependOnPublicBaseUrl, PublicBaseUrl};
use kernel::interfaces::crypto::{DependOnKeyEncryptor, DependOnPasswordProvider};
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::http_signing::DependOnHttpSigner;
use kernel::interfaces::read_model::{DependOnAccountQuery, DependOnProfileReadModel};
use kernel::interfaces::repository::{
    DependOnFollowRepository, DependOnImageRepository, DependOnRemoteAccountRepository,
    DependOnSigningKeyRepository, FollowRepository, RemoteAccountRepository,
};
use kernel::prelude::entity::{AccountId, FollowTargetId};
use kernel::KernelError;
use serde_json::Value;
use std::future::Future;

pub trait DeliverUpdatePersonUseCase:
    Sync
    + Send
    + DependOnAccountQuery
    + DependOnProfileReadModel
    + DependOnImageRepository
    + DependOnFollowRepository
    + DependOnRemoteAccountRepository
    + DependOnSigningKeyRepository
    + DependOnHttpSigner
    + DependOnPasswordProvider
    + DependOnKeyEncryptor
    + DependOnPublicBaseUrl
    + GetActorUseCase
{
    fn deliver_update_person(
        &self,
        account_id: &AccountId,
        account_nanoid: &str,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        async move {
            let actor = self
                .get_actor(GetActorDto {
                    account_nanoid: account_nanoid.to_string(),
                })
                .await?;
            let activity = update_person_activity(self.public_base_url(), &actor)?;
            let mut executor = self.database_connection().connection().await?;
            let followers = self
                .follow_repository()
                .find_followers(&mut executor, &FollowTargetId::from(account_id.clone()))
                .await?;
            for follow in followers {
                if follow.approved_at().is_none() {
                    continue;
                }
                let FollowTargetId::Remote(remote_id) = follow.source() else {
                    continue;
                };
                let Some(remote) = self
                    .remote_account_repository()
                    .find_by_id(&mut executor, remote_id)
                    .await?
                else {
                    continue;
                };
                let Some(inbox_url) = remote.inbox_url() else {
                    continue;
                };
                if let Err(error) = deliver_activity_to_inbox(
                    self,
                    account_id,
                    inbox_url,
                    &activity,
                    "Update(Person)",
                )
                .await
                {
                    tracing::warn!(
                        ?error,
                        inbox_url,
                        "Failed to deliver ActivityPub Update(Person)"
                    );
                }
            }
            Ok(())
        }
    }
}

fn update_person_activity(
    public_base_url: &PublicBaseUrl,
    actor: &Actor,
) -> error_stack::Result<Activity, KernelError> {
    let actor_url = actor.id.clone();
    let object = serde_json::to_value(actor).map_err(|error| {
        Report::new(KernelError::Internal)
            .attach_printable(format!("Failed to serialize Person actor: {error}"))
    })?;
    Ok(Activity {
        context: Some(Value::String(ACTIVITYSTREAMS_CONTEXT.to_string())),
        id: format!(
            "{}/activities/{}",
            public_base_url.as_str().trim_end_matches('/'),
            kernel::generate_id()
        ),
        type_: "Update".to_string(),
        actor: actor_url,
        object: Some(object),
        target: None,
        to: Some(vec![
            "https://www.w3.org/ns/activitystreams#Public".to_string()
        ]),
        cc: None,
    })
}
impl<T> DeliverUpdatePersonUseCase for T where
    T: Sync
        + Send
        + DependOnAccountQuery
        + DependOnProfileReadModel
        + DependOnImageRepository
        + DependOnFollowRepository
        + DependOnRemoteAccountRepository
        + DependOnSigningKeyRepository
        + DependOnHttpSigner
        + DependOnPasswordProvider
        + DependOnKeyEncryptor
        + DependOnPublicBaseUrl
        + GetActorUseCase
{
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::activitypub::{ActorImages, ActorUrlBuilder};

    #[test]
    fn update_person_activity_wraps_actor_as_public_update() {
        kernel::ensure_generator_initialized();
        let actor = Actor::new(
            &ActorUrlBuilder::new("https://local.example", "alice"),
            "alice",
            Some("Alice"),
            None,
            &ActorImages {
                icon_url: Some("https://media.example/avatar.png"),
                banner_url: Some("https://media.example/banner.png"),
            },
            "pem-content",
            "https://local.example/ap/accounts/alice#main-key",
        );

        let activity = update_person_activity(
            &PublicBaseUrl::new("https://local.example".to_string()),
            &actor,
        )
        .unwrap();

        assert_eq!(activity.type_, "Update");
        assert_eq!(activity.actor, "https://local.example/ap/accounts/alice");
        assert!(activity.id.starts_with("https://local.example/activities/"));
        assert_eq!(
            activity.to,
            Some(vec![
                "https://www.w3.org/ns/activitystreams#Public".to_string()
            ])
        );
        let object = activity.object.unwrap();
        assert_eq!(object["type"], "Person");
        assert_eq!(object["icon"]["url"], "https://media.example/avatar.png");
        assert_eq!(object["image"]["url"], "https://media.example/banner.png");
    }
}
