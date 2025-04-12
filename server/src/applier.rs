use crate::handler::Handler;
use account_applier::AccountApplier;
use auth_account_applier::AuthAccountApplier;
use kernel::interfaces::signal::Signal;
use kernel::prelude::entity::{AccountId, AuthAccountId};
use std::sync::Arc;

mod account_applier;
mod auth_account_applier;

pub(crate) struct ApplierContainer {
    account_applier: AccountApplier,
    auth_account_applier: AuthAccountApplier,
}

impl ApplierContainer {
    pub fn new(module: Arc<Handler>) -> Self {
        Self {
            account_applier: AccountApplier::new(module.clone()),
            auth_account_applier: AuthAccountApplier::new(module.clone()),
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

impl_signal!(AccountId, account_applier);
impl_signal!(AuthAccountId, auth_account_applier);
