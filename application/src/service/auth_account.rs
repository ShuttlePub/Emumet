use crate::transfer::auth_account::AuthAccountInfo;
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::modify::{
    AuthHostModifier, DependOnAuthAccountModifier, DependOnAuthHostModifier,
};
use kernel::interfaces::query::{
    AuthAccountQuery, AuthHostQuery, DependOnAuthAccountQuery, DependOnAuthHostQuery,
};
use kernel::prelude::entity::{
    AuthAccount, AuthAccountClientId, AuthAccountId, AuthHost, AuthHostId, AuthHostUrl,
};
use kernel::KernelError;

pub(crate) async fn get_auth_account<
    S: 'static
        + DependOnDatabaseConnection
        + DependOnAuthAccountQuery
        + DependOnAuthAccountModifier
        + DependOnAuthHostQuery
        + DependOnAuthHostModifier
        + ?Sized,
>(
    service: &S,
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
        let auth_account = AuthAccount::create(AuthAccountId::default(), host_id, client_id);
        todo!()
    };
    Ok(auth_account)
}
