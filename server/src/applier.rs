use crate::handler::Handler;
use auth_account_applier::AuthAccountApplier;
use kernel::interfaces::signal::Signal;
use kernel::prelude::entity::{AuthAccountId, MetadataId, ProfileId};
use metadata_applier::MetadataApplier;
use profile_applier::ProfileApplier;
use std::sync::Arc;

mod auth_account_applier;
mod metadata_applier;
mod profile_applier;

pub struct ApplierContainer {
    auth_account_applier: AuthAccountApplier,
    profile_applier: ProfileApplier,
    metadata_applier: MetadataApplier,
}

impl ApplierContainer {
    pub fn new(module: Arc<Handler>) -> Self {
        Self {
            auth_account_applier: AuthAccountApplier::new(module.clone()),
            profile_applier: ProfileApplier::new(module.clone()),
            metadata_applier: MetadataApplier::new(module.clone()),
        }
    }
}

macro_rules! impl_signal {
    ($type:ty, $field:ident) => {
        impl Signal<$type> for ApplierContainer {
            async fn emit(&self, signal_id: $type) -> error_stack::Result<(), kernel::KernelError> {
                self.$field.emit(signal_id).await
            }
        }
    };
}

impl_signal!(AuthAccountId, auth_account_applier);
impl_signal!(ProfileId, profile_applier);
impl_signal!(MetadataId, metadata_applier);
