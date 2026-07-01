//! Configuration for ActivityPub E2E tests.
//!
//! Provides a typed config helper so tests can reference base URLs from
//! environment variables instead of hardcoding `http://localhost:8080`.

/// E2E test configuration for ActivityPub federation scenarios.
pub struct ApE2eConfig {
    /// Base URL of the Emumet server (e.g. `http://localhost:8080`).
    pub server_base_url: String,
    /// Public base URL exposed to peers (typically same as `server_base_url`).
    pub public_base_url: String,
}

/// Return an [`ApE2eConfig`] populated from environment variables.
///
/// Variables and defaults:
///
/// | Variable | Default |
/// |---|---|
/// | `EMUMET_E2E_SERVER_BASE_URL` | `http://localhost:8080` |
/// | `EMUMET_E2E_PUBLIC_BASE_URL` | `http://localhost:8080` |
pub fn ap_e2e_config() -> ApE2eConfig {
    ApE2eConfig {
        server_base_url: std::env::var("EMUMET_E2E_SERVER_BASE_URL")
            .unwrap_or_else(|_| "http://localhost:8080".to_string()),
        public_base_url: std::env::var("EMUMET_E2E_PUBLIC_BASE_URL")
            .unwrap_or_else(|_| "http://localhost:8080".to_string()),
    }
}
