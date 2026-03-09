use error_stack::Report;
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::modify::{
    AuthAccountModifier, DependOnAuthAccountModifier, DependOnAuthHostModifier,
};
use kernel::interfaces::query::{
    AuthAccountQuery, DependOnAuthAccountQuery, DependOnAuthHostQuery, DependOnEventQuery,
    EventQuery,
};
use kernel::prelude::entity::{AuthAccount, AuthAccountId, EventId};
use kernel::KernelError;
use std::future::Future;

pub trait UpdateAuthAccount:
    'static
    + DependOnDatabaseConnection
    + DependOnAuthAccountQuery
    + DependOnAuthAccountModifier
    + DependOnAuthHostQuery
    + DependOnAuthHostModifier
    + DependOnEventQuery
{
    fn update_auth_account(
        &self,
        auth_account_id: AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> {
        async move {
            let mut transaction = self.database_connection().begin_transaction().await?;
            let auth_account = self
                .auth_account_query()
                .find_by_id(&mut transaction, &auth_account_id)
                .await?;
            if let Some(auth_account) = auth_account {
                let event_id = EventId::from(auth_account.id().clone());
                let events = self
                    .event_query()
                    .find_by_id(&mut transaction, &event_id, Some(auth_account.version()))
                    .await?;
                if events
                    .last()
                    .map(|event| &event.version != auth_account.version())
                    .unwrap_or_else(|| false)
                {
                    let mut auth_account = Some(auth_account);
                    for event in events {
                        AuthAccount::apply(&mut auth_account, event)?;
                    }
                    if let Some(auth_account) = auth_account {
                        self.auth_account_modifier()
                            .update(&mut transaction, &auth_account)
                            .await?;
                    } else {
                        return Err(Report::new(KernelError::Internal)
                            .attach_printable("Failed to get auth account"));
                    }
                }
                Ok(())
            } else {
                Err(Report::new(KernelError::Internal).attach_printable(format!(
                    "Failed to get target auth account: {auth_account_id:?}"
                )))
            }
        }
    }
}

impl<T> UpdateAuthAccount for T where
    T: 'static
        + DependOnDatabaseConnection
        + DependOnAuthAccountQuery
        + DependOnAuthAccountModifier
        + DependOnAuthHostQuery
        + DependOnAuthHostModifier
        + DependOnEventQuery
{
}
