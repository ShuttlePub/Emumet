use crate::transfer::auth_account::AuthAccountInfo;
use error_stack::Report;
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::modify::{
    AuthAccountModifier, AuthHostModifier, DependOnAuthAccountModifier, DependOnAuthHostModifier,
    DependOnEventModifier, EventModifier,
};
use kernel::interfaces::query::{
    AuthAccountQuery, AuthHostQuery, DependOnAuthAccountQuery, DependOnAuthHostQuery,
    DependOnEventQuery, EventQuery,
};
use kernel::interfaces::signal::Signal;
use kernel::prelude::entity::{
    AuthAccount, AuthAccountClientId, AuthAccountId, AuthHost, AuthHostId, AuthHostUrl, EventId,
};
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
    /// Update the auth account from events
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

/// Get an auth account by the host URL and client ID.
/// If the auth account does not exist, it will be created.
pub(crate) async fn get_auth_account<
    S: 'static
        + DependOnDatabaseConnection
        + DependOnAuthAccountQuery
        + DependOnAuthAccountModifier
        + DependOnAuthHostQuery
        + DependOnAuthHostModifier
        + DependOnEventModifier
        + DependOnEventQuery
        + ?Sized,
>(
    service: &S,
    signal: &impl Signal<AuthAccountId>,
    AuthAccountInfo {
        host_url: host,
        client_id,
    }: AuthAccountInfo,
) -> error_stack::Result<AuthAccount, KernelError> {
    let client_id = AuthAccountClientId::new(client_id);
    let mut transaction = service.database_connection().begin_transaction().await?;
    let auth_account = service
        .auth_account_query()
        .find_by_client_id(&mut transaction, &client_id)
        .await?;
    let auth_account = if let Some(auth_account) = auth_account {
        auth_account
    } else {
        let url = AuthHostUrl::new(host);
        let auth_host = service
            .auth_host_query()
            .find_by_url(&mut transaction, &url)
            .await?;
        let auth_host = if let Some(auth_host) = auth_host {
            auth_host
        } else {
            let auth_host = AuthHost::new(AuthHostId::default(), url);
            service
                .auth_host_modifier()
                .create(&mut transaction, &auth_host)
                .await?;
            auth_host
        };
        let host_id = auth_host.into_destruct().id;
        let auth_account_id = AuthAccountId::default();
        let create_command = AuthAccount::create(auth_account_id.clone(), host_id, client_id);
        let create_event = service
            .event_modifier()
            .persist_and_transform(&mut transaction, create_command)
            .await?;
        signal.emit(auth_account_id.clone()).await?;
        let mut auth_account = None;
        AuthAccount::apply(&mut auth_account, create_event)?;
        if let Some(auth_account) = auth_account {
            service
                .auth_account_modifier()
                .create(&mut transaction, &auth_account)
                .await?;
            auth_account
        } else {
            return Err(Report::new(KernelError::Internal)
                .attach_printable("Failed to create auth account"));
        }
    };
    Ok(auth_account)
}
