use axum_keycloak_auth::instance::{KeycloakAuthInstance, KeycloakConfig};

const KEYCLOAK_SERVER_KEY: &str = "KEYCLOAK_SERVER";
const KEYCLOAK_REALM_KEY: &str = "KEYCLOAK_REALM";

pub fn create_keycloak_instance() -> KeycloakAuthInstance {
    let server =
        dotenvy::var(KEYCLOAK_SERVER_KEY).unwrap_or_else(|_| "http://localhost:18080/".to_string());
    let realm = dotenvy::var(KEYCLOAK_REALM_KEY).unwrap_or_else(|_| "MyRealm".to_string());
    log::info!("Keycloak info: server={server}, realm={realm}");
    KeycloakAuthInstance::new(
        KeycloakConfig::builder()
            .server(server.parse().unwrap())
            .realm(realm)
            .build(),
    )
}

#[macro_export]
macro_rules! expect_role {
    ($token: expr, $req: expr) => {
        let expected_roles =
            $crate::route::to_permission_strings(&$req.uri().to_string(), $req.method().as_str());
        let role_result = expected_roles
            .iter()
            .map(|role| axum_keycloak_auth::role::ExpectRoles::expect_roles($token, &[role]))
            .collect::<Vec<Result<_, _>>>();
        if !role_result.iter().any(|r| r.is_ok()) {
            return Err($crate::error::ErrorStatus::from((
                StatusCode::FORBIDDEN,
                format!(
                    "Permission denied: required roles(any) = {:?}",
                    expected_roles
                ),
            )));
        }
    };
}
