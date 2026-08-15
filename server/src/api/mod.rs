//! Route-facing facade newtypes (ADR 0006 decision 7).
//!
//! Facades expose use-case methods only; they never implement `DependOn*`
//! and never hand raw ports or executors to route code.

pub(crate) mod account;
pub(crate) mod activitypub;
pub(crate) mod admin_account;
pub(crate) mod me;
pub(crate) mod oauth2;
pub(crate) mod signing;

use crate::auth::OidcAuthInfo;
use crate::handler::AppModule;
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::repository::{
    AuthAccountRepository, AuthHostRepository, DependOnAuthAccountRepository,
    DependOnAuthHostRepository,
};
use kernel::prelude::entity::{
    AuthAccountClientId, AuthAccountId, AuthHost, AuthHostId, AuthHostUrl,
};
use kernel::KernelError;

pub(crate) use account::AccountApi;
pub(crate) use activitypub::ActivityPubApi;
pub(crate) use admin_account::AdminAccountApi;
pub(crate) use me::MeApi;
pub(crate) use oauth2::OAuth2Api;
pub(crate) use signing::SigningApi;

pub(crate) async fn resolve_auth_account_id(
    app: &AppModule,
    auth_info: OidcAuthInfo,
) -> error_stack::Result<AuthAccountId, KernelError> {
    let client_id = AuthAccountClientId::new(auth_info.subject);
    let mut executor = app.database_connection().connection().await?;
    let url = AuthHostUrl::new(auth_info.issuer);
    let auth_host = app
        .auth_host_repository()
        .find_by_url(&mut executor, &url)
        .await?;
    let auth_host = if let Some(auth_host) = auth_host {
        auth_host
    } else {
        let auth_host = AuthHost::new(AuthHostId::default(), url);
        app.auth_host_repository()
            .create(&mut executor, &auth_host)
            .await?;
        auth_host
    };
    let host_id = auth_host.into_destruct().id;
    let auth_account = app
        .auth_account_repository()
        .find_or_create(&mut executor, &host_id, &client_id)
        .await?;
    Ok(auth_account.id().clone())
}
