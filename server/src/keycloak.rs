use crate::handler::AppModule;
use adapter::processor::auth_account::{
    AuthAccountCommandProcessor, AuthAccountQueryProcessor, DependOnAuthAccountCommandProcessor,
    DependOnAuthAccountQueryProcessor,
};
use axum_keycloak_auth::decode::KeycloakToken;
use axum_keycloak_auth::instance::{KeycloakAuthInstance, KeycloakConfig};
use axum_keycloak_auth::role::Role;
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::modify::{AuthHostModifier, DependOnAuthHostModifier};
use kernel::interfaces::query::{AuthHostQuery, DependOnAuthHostQuery};
use kernel::prelude::entity::{
    AuthAccountClientId, AuthAccountId, AuthHost, AuthHostId, AuthHostUrl,
};
use kernel::KernelError;
use std::sync::LazyLock;

const KEYCLOAK_SERVER_KEY: &str = "KEYCLOAK_SERVER";
static SERVER_URL: LazyLock<String> = LazyLock::new(|| {
    dotenvy::var(KEYCLOAK_SERVER_KEY).unwrap_or_else(|_| "http://localhost:18080/".to_string())
});

const KEYCLOAK_REALM_KEY: &str = "KEYCLOAK_REALM";

pub fn create_keycloak_instance() -> KeycloakAuthInstance {
    let server = &*SERVER_URL;
    let realm = dotenvy::var(KEYCLOAK_REALM_KEY).unwrap_or_else(|_| "MyRealm".to_string());
    log::info!("Keycloak info: server={server}, realm={realm}");
    KeycloakAuthInstance::new(
        KeycloakConfig::builder()
            .server(server.parse().unwrap())
            .realm(realm)
            .build(),
    )
}

#[derive(Debug)]
pub struct KeycloakAuthAccount {
    host_url: String,
    client_id: String,
}

impl<T: Role> From<KeycloakToken<T>> for KeycloakAuthAccount {
    fn from(token: KeycloakToken<T>) -> Self {
        Self {
            host_url: SERVER_URL.clone(),
            client_id: token.subject,
        }
    }
}

pub async fn resolve_auth_account_id(
    app: &AppModule,
    auth_info: KeycloakAuthAccount,
) -> error_stack::Result<AuthAccountId, KernelError> {
    let client_id = AuthAccountClientId::new(auth_info.client_id);
    let mut executor = app.database_connection().begin_transaction().await?;
    let auth_account = app
        .auth_account_query_processor()
        .find_by_client_id(&mut executor, &client_id)
        .await?;
    let auth_account = if let Some(auth_account) = auth_account {
        auth_account
    } else {
        let url = AuthHostUrl::new(auth_info.host_url);
        let auth_host = app
            .auth_host_query()
            .find_by_url(&mut executor, &url)
            .await?;
        let auth_host = if let Some(auth_host) = auth_host {
            auth_host
        } else {
            let auth_host = AuthHost::new(AuthHostId::default(), url);
            app.auth_host_modifier()
                .create(&mut executor, &auth_host)
                .await?;
            auth_host
        };
        let host_id = auth_host.into_destruct().id;
        app.auth_account_command_processor()
            .create(&mut executor, host_id, client_id)
            .await?
    };
    Ok(auth_account.id().clone())
}

#[macro_export]
macro_rules! expect_role {
    ($token: expr, $uri: expr, $method: expr) => {
        let expected_roles =
            $crate::route::to_permission_strings(&$uri.to_string(), $method.as_str());
        let role_result = expected_roles
            .iter()
            .map(|role| axum_keycloak_auth::role::ExpectRoles::expect_roles($token, &[role]))
            .collect::<Vec<Result<_, _>>>();
        if !role_result.iter().any(|r| r.is_ok()) {
            return Err($crate::error::ErrorStatus::from((
                axum::http::StatusCode::FORBIDDEN,
                format!(
                    "Permission denied: required roles(any) = {:?}",
                    expected_roles
                ),
            )));
        }
    };
}
