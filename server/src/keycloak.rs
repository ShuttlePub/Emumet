use application::transfer::auth_account::AuthAccountInfo;
use axum_keycloak_auth::decode::KeycloakToken;
use axum_keycloak_auth::instance::{KeycloakAuthInstance, KeycloakConfig};
use axum_keycloak_auth::role::Role;
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

impl From<KeycloakAuthAccount> for AuthAccountInfo {
    fn from(val: KeycloakAuthAccount) -> Self {
        AuthAccountInfo {
            host_url: val.host_url,
            client_id: val.client_id,
        }
    }
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
