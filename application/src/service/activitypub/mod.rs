mod fetch;
mod profile_actor;
mod profile_inbound;
pub(crate) mod remote_actor;

pub use profile_actor::{GetProfileActorUseCase, GetProfileWebFingerUseCase};
pub use profile_inbound::ProfileInboxUseCase;
#[cfg(any(test, feature = "test-mode"))]
pub use remote_actor::inject_test_remote_actor;

pub(super) const ACTIVITY_JSON: &str = "application/activity+json";
pub(super) const ACTIVITYSTREAMS_CONTEXT: &str = "https://www.w3.org/ns/activitystreams";
