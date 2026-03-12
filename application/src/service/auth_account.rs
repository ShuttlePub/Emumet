use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::event_store::{AuthAccountEventStore, DependOnAuthAccountEventStore};
use kernel::interfaces::read_model::{AuthAccountReadModel, DependOnAuthAccountReadModel};
use kernel::prelude::entity::{AuthAccount, AuthAccountId, EventId};
use kernel::KernelError;
use std::future::Future;

pub trait UpdateAuthAccount:
    'static + DependOnDatabaseConnection + DependOnAuthAccountReadModel + DependOnAuthAccountEventStore
{
    fn update_auth_account(
        &self,
        auth_account_id: AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> {
        async move {
            let mut transaction = self.database_connection().begin_transaction().await?;
            let existing = self
                .auth_account_read_model()
                .find_by_id(&mut transaction, &auth_account_id)
                .await?;
            let event_id = EventId::from(auth_account_id.clone());

            if let Some(auth_account) = existing {
                let events = self
                    .auth_account_event_store()
                    .find_by_id(&mut transaction, &event_id, Some(auth_account.version()))
                    .await?;
                if events
                    .last()
                    .map(|event| &event.version != auth_account.version())
                    .unwrap_or(false)
                {
                    let mut auth_account = Some(auth_account);
                    for event in events {
                        AuthAccount::apply(&mut auth_account, event)?;
                    }
                    if let Some(auth_account) = auth_account {
                        self.auth_account_read_model()
                            .update(&mut transaction, &auth_account)
                            .await?;
                    }
                }
            } else {
                let events = self
                    .auth_account_event_store()
                    .find_by_id(&mut transaction, &event_id, None)
                    .await?;
                if !events.is_empty() {
                    let mut auth_account = None;
                    for event in events {
                        AuthAccount::apply(&mut auth_account, event)?;
                    }
                    if let Some(auth_account) = auth_account {
                        self.auth_account_read_model()
                            .create(&mut transaction, &auth_account)
                            .await?;
                    }
                }
            }
            Ok(())
        }
    }
}

impl<T> UpdateAuthAccount for T where
    T: 'static
        + DependOnDatabaseConnection
        + DependOnAuthAccountReadModel
        + DependOnAuthAccountEventStore
{
}
