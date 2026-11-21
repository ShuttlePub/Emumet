//! Start-up step of the instance identity (P1 post-relay, K1).
//!
//! Runs after the migrations and the Profile key provisioning, before any worker or listener is
//! started: the identity and the first key are created only when neither exists, and every other
//! stored state (a different `PUBLIC_BASE_URL`, an undecryptable or damaged key) stops the start-up
//! instead of being repaired by generating a new key.

use crate::handler::AppModule;
use application::instance_identity::{EnsureInstanceIdentityUseCase, InstanceIdentityReport};
use kernel::KernelError;

pub async fn provision_instance_identity(
    app: &AppModule,
) -> error_stack::Result<InstanceIdentityReport, KernelError> {
    let report = app.ensure_instance_identity().await?;
    if report.created {
        tracing::info!(key_id = %report.key_id_uri, "created the instance identity and its signing key");
    } else {
        tracing::info!(key_id = %report.key_id_uri, "the instance identity is unchanged; nothing generated");
    }
    Ok(report)
}
