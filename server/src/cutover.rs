//! Start-up steps of the Profile federation cutover (ADR profile-federation-in-place-reset).
//!
//! The destructive part of the cutover (initialising the legacy Account federation data) is a
//! versioned SQL migration that runs when the new binary starts (`AppModule::new`, one
//! transaction, refused while any Profile has no username, recorded in
//! `profile_federation_resets` so it never runs twice). What SQL cannot do — generating the
//! Profile actor keys, which need the master key password — happens here, right after the
//! migrations and before any route is served or any worker is started.

use crate::handler::AppModule;
use application::profile_signing_key::{
    EnsureProfileSigningKeysUseCase, ProfileKeyProvisioningReport,
};
use kernel::KernelError;

/// Generates the actor key of every Profile that has none (idempotent: Profiles that already
/// have an active key are never touched, so a restart or a second process never regenerates a
/// key). Fails — and the server does not start — when a key cannot be generated, because a
/// Profile without a key is not an actor (404 on its actor document, undeliverable follows).
pub async fn provision_profile_keys(
    app: &AppModule,
) -> error_stack::Result<ProfileKeyProvisioningReport, KernelError> {
    let report = app.ensure_profile_signing_keys().await?;
    if report.created > 0 {
        tracing::info!(
            created = report.created,
            "generated the one-time actor keys of Profiles that had none"
        );
    } else {
        tracing::info!("every Profile already has an actor key; nothing generated");
    }
    Ok(report)
}

#[cfg(test)]
#[path = "cutover_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "profile_lifecycle_tests.rs"]
mod lifecycle_tests;

#[cfg(test)]
#[path = "profile_moderation_tests.rs"]
mod moderation_tests;
