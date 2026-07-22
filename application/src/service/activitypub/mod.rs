mod actor;
mod collections;
mod delivery;
mod fetch;
mod inbox;
mod outbound_follow;
mod outbox;
pub(crate) mod remote_actor;

use kernel::activitypub::ActorUrlBuilder;
use kernel::interfaces::config::PublicBaseUrl;

pub use actor::{GetActorUseCase, GetWebFingerUseCase};
pub use collections::GetFollowersCollectionUseCase;
pub use inbox::InboxUseCase;
pub use outbound_follow::SendFollowUseCase;
pub use outbox::{GetOutboxUseCase, StoreOutboxActivityUseCase};
#[cfg(any(test, feature = "test-mode"))]
pub use remote_actor::inject_test_remote_actor;

pub(super) const ACTIVITY_JSON: &str = "application/activity+json";
pub(super) const ACTIVITYSTREAMS_CONTEXT: &str = "https://www.w3.org/ns/activitystreams";

pub(super) fn local_actor_url(public_base_url: &PublicBaseUrl, account_nanoid: &str) -> String {
    ActorUrlBuilder::new(public_base_url.as_str(), account_nanoid).actor_id()
}
