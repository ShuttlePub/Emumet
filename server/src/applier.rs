use crate::handler::Handler;
use kernel::interfaces::signal::Signal;
use kernel::prelude::entity::{MetadataId, ProfileId};
use metadata_applier::MetadataApplier;
use profile_applier::ProfileApplier;
use std::sync::Arc;

mod metadata_applier;
mod profile_applier;

pub struct ApplierContainer {
    profile_applier: ProfileApplier,
    metadata_applier: MetadataApplier,
}

impl ApplierContainer {
    pub fn new(module: Arc<Handler>) -> Self {
        Self {
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

impl_signal!(ProfileId, profile_applier);
impl_signal!(MetadataId, metadata_applier);
