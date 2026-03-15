use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use jsonwebtoken::jwk::KeyAlgorithm;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};

// ---------------------------------------------------------------------------
// OidcConfig
// ---------------------------------------------------------------------------

pub struct OidcConfig {
    pub issuer_url: String,
    pub expected_audience: String,
    /// Minimum interval between JWKS re-fetches. Set to 0 in tests.
    pub jwks_refetch_interval_secs: u64,
}

impl OidcConfig {
    /// Initialize from environment variables `HYDRA_ISSUER_URL` and `EXPECTED_AUDIENCE`.
    pub fn from_env() -> Self {
        let issuer_url = dotenvy::var("HYDRA_ISSUER_URL").unwrap_or_else(|_| {
            let default = "http://localhost:4444".to_string();
            tracing::warn!("HYDRA_ISSUER_URL not set, using default: {default}");
            default
        });
        let expected_audience = dotenvy::var("EXPECTED_AUDIENCE").unwrap_or_else(|_| {
            let default = "emumet".to_string();
            tracing::warn!("EXPECTED_AUDIENCE not set, using default: {default}");
            default
        });
        Self {
            issuer_url,
            expected_audience,
            jwks_refetch_interval_secs: 300,
        }
    }
}

// ---------------------------------------------------------------------------
// AuthClaims
// ---------------------------------------------------------------------------

/// JWT claims issued by Hydra.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct AuthClaims {
    pub iss: String,
    pub sub: String,
    /// `aud` may be a single string or an array of strings.
    pub aud: OneOrMany,
    pub exp: u64,
}

/// Represents a JSON value that can be either a single string or a list of strings.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    #[cfg(test)]
    pub fn contains(&self, value: &str) -> bool {
        match self {
            OneOrMany::One(s) => s == value,
            OneOrMany::Many(v) => v.iter().any(|s| s == value),
        }
    }
}

// ---------------------------------------------------------------------------
// OidcAuthInfo
// ---------------------------------------------------------------------------

/// Extracted auth info consumed by `resolve_auth_account_id` (task 5.1).
pub struct OidcAuthInfo {
    /// Hydra issuer URL → used as `AuthHost.url`
    pub issuer: String,
    /// Kratos identity UUID → used as `AuthAccount.client_id`
    pub subject: String,
}

impl From<AuthClaims> for OidcAuthInfo {
    fn from(claims: AuthClaims) -> Self {
        Self {
            issuer: claims.iss,
            subject: claims.sub,
        }
    }
}

// ---------------------------------------------------------------------------
// OIDC Discovery response
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct OidcDiscovery {
    jwks_uri: String,
}

// ---------------------------------------------------------------------------
// JwkSet wrapper (re-export from jsonwebtoken)
// ---------------------------------------------------------------------------

pub use jsonwebtoken::jwk::JwkSet;

// ---------------------------------------------------------------------------
// JwksCache
// ---------------------------------------------------------------------------

struct JwksCacheInner {
    jwks: Option<JwkSet>,
    jwks_uri: Option<String>,
    last_fetch: Instant,
}

pub struct JwksCache {
    inner: RwLock<JwksCacheInner>,
    /// Serialises refresh attempts to prevent thundering herd.
    refresh_mutex: Mutex<()>,
    issuer_url: String,
    http_client: reqwest::Client,
    min_refetch_interval: Duration,
}

impl JwksCache {
    pub fn new(issuer_url: String, min_refetch_interval: Duration) -> Self {
        let initial_last_fetch = Instant::now()
            .checked_sub(min_refetch_interval + Duration::from_secs(1))
            .unwrap_or_else(Instant::now);

        Self {
            inner: RwLock::new(JwksCacheInner {
                jwks: None,
                jwks_uri: None,
                last_fetch: initial_last_fetch,
            }),
            refresh_mutex: Mutex::new(()),
            issuer_url,
            http_client: reqwest::Client::new(),
            min_refetch_interval,
        }
    }

    /// Construct a `JwksCache` with a pre-populated `JwkSet` (test helper).
    /// The refetch interval is set to zero so re-fetches are always eligible.
    #[cfg(test)]
    pub fn new_with_jwks(issuer_url: String, jwks: JwkSet) -> Self {
        let past = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or_else(Instant::now);
        Self {
            inner: RwLock::new(JwksCacheInner {
                jwks: Some(jwks),
                jwks_uri: None,
                last_fetch: past,
            }),
            refresh_mutex: Mutex::new(()),
            issuer_url,
            http_client: reqwest::Client::new(),
            min_refetch_interval: Duration::from_secs(0),
        }
    }

    /// Attempt to initialise the cache by performing OIDC Discovery and fetching
    /// the JWKS. Failures are logged but do not panic (lazy initialisation).
    pub async fn try_init(&self) {
        let discovery_url = format!(
            "{}/.well-known/openid-configuration",
            self.issuer_url.trim_end_matches('/')
        );

        let discovery: OidcDiscovery = match self.http_client.get(&discovery_url).send().await {
            Ok(resp) => match resp.json().await {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!("JwksCache: failed to parse OIDC discovery: {e}");
                    return;
                }
            },
            Err(e) => {
                tracing::warn!("JwksCache: OIDC discovery request failed ({discovery_url}): {e}");
                return;
            }
        };

        let jwks_uri = discovery.jwks_uri.clone();
        self.fetch_jwks_inner(&jwks_uri).await;
        // Update jwks_uri after successful discovery.
        self.inner.write().await.jwks_uri = Some(jwks_uri);
    }

    /// Fetch JWKS from the given URI and update the cache atomically.
    async fn fetch_jwks_inner(&self, jwks_uri: &str) {
        match self.http_client.get(jwks_uri).send().await {
            Ok(resp) => match resp.json::<JwkSet>().await {
                Ok(jwks) => {
                    let mut inner = self.inner.write().await;
                    inner.jwks = Some(jwks);
                    inner.last_fetch = Instant::now();
                    tracing::info!("JwksCache: JWKS refreshed from {jwks_uri}");
                }
                Err(e) => {
                    tracing::warn!("JwksCache: failed to parse JWKS response: {e}");
                }
            },
            Err(e) => {
                tracing::warn!("JwksCache: JWKS fetch failed ({jwks_uri}): {e}");
            }
        }
    }

    /// Look up a decoding key by `kid`.
    ///
    /// If the key is not found **and** the minimum refetch interval has elapsed,
    /// the JWKS is re-fetched once before returning. A mutex ensures only one
    /// concurrent refresh.
    pub async fn get_key(&self, kid: &str) -> Option<DecodingKey> {
        // First attempt: check current cache.
        if let Some(key) = self.key_from_cache(kid).await {
            return Some(key);
        }

        // Acquire refresh mutex to prevent thundering herd.
        let _guard = self.refresh_mutex.lock().await;

        // Re-check cache: another task may have refreshed while we waited.
        if let Some(key) = self.key_from_cache(kid).await {
            return Some(key);
        }

        // Check if we are allowed to re-fetch.
        let elapsed = self.inner.read().await.last_fetch.elapsed();

        if elapsed >= self.min_refetch_interval {
            tracing::info!("JwksCache: kid '{kid}' not found – re-fetching JWKS");

            let jwks_uri = self.inner.read().await.jwks_uri.clone();

            if let Some(uri) = jwks_uri {
                self.fetch_jwks_inner(&uri).await;
            } else {
                self.try_init().await;
            }

            // Second attempt after re-fetch.
            return self.key_from_cache(kid).await;
        }

        None
    }

    /// Extract a `DecodingKey` for the given `kid` from the current in-memory cache.
    /// Rejects JWKs whose `alg` field does not match RS256.
    async fn key_from_cache(&self, kid: &str) -> Option<DecodingKey> {
        let inner = self.inner.read().await;
        let jwks = inner.jwks.as_ref()?;
        let jwk = jwks.find(kid)?;
        if let Some(alg) = &jwk.common.key_algorithm {
            if *alg != KeyAlgorithm::RS256 {
                tracing::warn!("JwksCache: JWK kid={kid} has unexpected algorithm {alg:?}");
                return None;
            }
        }
        DecodingKey::from_jwk(jwk).ok()
    }
}

// ---------------------------------------------------------------------------
// auth_middleware (axum middleware layer)
// ---------------------------------------------------------------------------

/// Axum middleware that validates Bearer JWTs and inserts
/// [`Extension<AuthClaims>`] into the request.
///
/// Usage with router:
/// ```ignore
/// let state = (config, jwks_cache);
/// router.layer(axum::middleware::from_fn_with_state(state, auth_middleware))
/// ```
pub async fn auth_middleware(
    State((config, jwks_cache)): State<(Arc<OidcConfig>, Arc<JwksCache>)>,
    mut request: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    auth_middleware_core(config, jwks_cache, &mut request).await?;
    Ok(next.run(request).await)
}

/// Core middleware logic: extracts and validates the Bearer token, then
/// inserts `Extension<AuthClaims>` into the request extensions.
async fn auth_middleware_core(
    config: Arc<OidcConfig>,
    jwks_cache: Arc<JwksCache>,
    request: &mut Request<Body>,
) -> Result<(), StatusCode> {
    // 1. Extract the Bearer token from the Authorization header.
    let token = extract_bearer_token(request)?;

    // 2. Decode the JWT header to obtain the `kid`.
    let header = decode_header(token).map_err(|e| {
        tracing::warn!("auth_middleware: failed to decode JWT header: {e}");
        StatusCode::UNAUTHORIZED
    })?;

    let kid = header.kid.ok_or_else(|| {
        tracing::warn!("auth_middleware: JWT header missing 'kid'");
        StatusCode::UNAUTHORIZED
    })?;

    // 3. Fetch the public key from JWKS cache.
    let decoding_key = jwks_cache.get_key(&kid).await.ok_or_else(|| {
        tracing::warn!("auth_middleware: no JWKS key found for kid='{kid}'");
        StatusCode::UNAUTHORIZED
    })?;

    // 4. Validate the JWT (RS256, iss, aud, exp).
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_audience(&[&config.expected_audience]);
    validation.set_issuer(&[&config.issuer_url]);

    let token_data = decode::<AuthClaims>(token, &decoding_key, &validation).map_err(|e| {
        tracing::warn!("auth_middleware: JWT validation failed: {e}");
        StatusCode::UNAUTHORIZED
    })?;

    // 5. Insert claims as request extension.
    request.extensions_mut().insert(token_data.claims);

    Ok(())
}

// ---------------------------------------------------------------------------
// resolve_auth_account_id
// ---------------------------------------------------------------------------

use crate::handler::AppModule;
use adapter::processor::auth_account::{
    AuthAccountCommandProcessor, AuthAccountQueryProcessor, DependOnAuthAccountCommandProcessor,
    DependOnAuthAccountQueryProcessor,
};
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::repository::{AuthHostRepository, DependOnAuthHostRepository};
use kernel::prelude::entity::{
    AuthAccountClientId, AuthAccountId, AuthHost, AuthHostId, AuthHostUrl,
};
use kernel::KernelError;

pub async fn resolve_auth_account_id(
    app: &AppModule,
    auth_info: OidcAuthInfo,
) -> error_stack::Result<AuthAccountId, KernelError> {
    let client_id = AuthAccountClientId::new(auth_info.subject);
    let mut executor = app.database_connection().begin_transaction().await?;
    let auth_account = app
        .auth_account_query_processor()
        .find_by_client_id(&mut executor, &client_id)
        .await?;
    let auth_account = if let Some(auth_account) = auth_account {
        auth_account
    } else {
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
        app.auth_account_command_processor()
            .create(&mut executor, host_id, client_id)
            .await?
    };
    Ok(auth_account.id().clone())
}

/// Extract the raw Bearer token string from the `Authorization` header.
fn extract_bearer_token(request: &Request<Body>) -> Result<&str, StatusCode> {
    let header_value = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .ok_or_else(|| {
            tracing::warn!("auth_middleware: missing Authorization header");
            StatusCode::UNAUTHORIZED
        })?;

    let header_str = header_value.to_str().map_err(|_| {
        tracing::warn!("auth_middleware: Authorization header is not valid UTF-8");
        StatusCode::UNAUTHORIZED
    })?;

    let token = header_str.strip_prefix("Bearer ").ok_or_else(|| {
        tracing::warn!("auth_middleware: Authorization header does not start with 'Bearer '");
        StatusCode::UNAUTHORIZED
    })?;

    Ok(token)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::jwk::{
        AlgorithmParameters, CommonParameters, Jwk, JwkSet, KeyAlgorithm, PublicKeyUse,
        RSAKeyParameters,
    };
    use jsonwebtoken::{encode, EncodingKey, Header};
    use rsa::pkcs1::EncodeRsaPrivateKey;
    use rsa::RsaPrivateKey;
    use std::time::{SystemTime, UNIX_EPOCH};

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    fn unix_now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    struct TestKeys {
        encoding_key: EncodingKey,
        jwk_set: JwkSet,
        kid: String,
    }

    /// Generate a fresh 2048-bit RSA key pair and wrap it as a `JwkSet`.
    fn generate_test_keys() -> TestKeys {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
        use rsa::traits::PublicKeyParts;

        let mut rng = rand::thread_rng();
        let private_key = RsaPrivateKey::new(&mut rng, 2048).expect("generate RSA key");

        // PEM → EncodingKey
        let pem = private_key
            .to_pkcs1_pem(rsa::pkcs8::LineEnding::LF)
            .expect("encode pkcs1 pem");
        let encoding_key =
            EncodingKey::from_rsa_pem(pem.as_bytes()).expect("parse EncodingKey from PEM");

        // Build JWK from RSA public key components.
        let pub_key = private_key.to_public_key();
        let n = URL_SAFE_NO_PAD.encode(pub_key.n().to_bytes_be());
        let e = URL_SAFE_NO_PAD.encode(pub_key.e().to_bytes_be());

        let kid = "test-key-1".to_string();
        let jwk = Jwk {
            common: CommonParameters {
                public_key_use: Some(PublicKeyUse::Signature),
                key_id: Some(kid.clone()),
                key_algorithm: Some(KeyAlgorithm::RS256),
                ..Default::default()
            },
            algorithm: AlgorithmParameters::RSA(RSAKeyParameters {
                n,
                e,
                ..Default::default()
            }),
        };
        let jwk_set = JwkSet { keys: vec![jwk] };

        TestKeys {
            encoding_key,
            jwk_set,
            kid,
        }
    }

    fn make_claims(iss: &str, aud: &str, sub: &str, exp_offset_secs: i64) -> AuthClaims {
        let exp = (unix_now() as i64 + exp_offset_secs) as u64;
        AuthClaims {
            iss: iss.to_string(),
            sub: sub.to_string(),
            aud: OneOrMany::One(aud.to_string()),
            exp,
        }
    }

    fn encode_jwt(claims: &AuthClaims, encoding_key: &EncodingKey, kid: &str) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(kid.to_string());
        encode(&header, claims, encoding_key).expect("encode JWT")
    }

    fn make_config(issuer: &str, audience: &str) -> Arc<OidcConfig> {
        Arc::new(OidcConfig {
            issuer_url: issuer.to_string(),
            expected_audience: audience.to_string(),
            jwks_refetch_interval_secs: 0,
        })
    }

    async fn validate(
        config: Arc<OidcConfig>,
        cache: Arc<JwksCache>,
        token: &str,
    ) -> Result<AuthClaims, StatusCode> {
        let mut req: Request<Body> = Request::builder()
            .header("Authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        auth_middleware_core(config, cache, &mut req).await?;
        Ok(req.extensions().get::<AuthClaims>().unwrap().clone())
    }

    // -----------------------------------------------------------------------
    // Test cases
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn valid_jwt_succeeds() {
        let keys = generate_test_keys();
        let issuer = "https://hydra.example.com";
        let audience = "emumet";
        let config = make_config(issuer, audience);
        let cache = Arc::new(JwksCache::new_with_jwks(issuer.to_string(), keys.jwk_set));

        let claims = make_claims(issuer, audience, "kratos-uuid-123", 3600);
        let token = encode_jwt(&claims, &keys.encoding_key, &keys.kid);

        let result = validate(config, cache, &token).await;
        assert!(result.is_ok(), "expected Ok, got {result:?}");
        let decoded = result.unwrap();
        assert_eq!(decoded.sub, "kratos-uuid-123");
        assert_eq!(decoded.iss, issuer);
    }

    #[tokio::test]
    async fn expired_jwt_fails() {
        let keys = generate_test_keys();
        let issuer = "https://hydra.example.com";
        let audience = "emumet";
        let config = make_config(issuer, audience);
        let cache = Arc::new(JwksCache::new_with_jwks(issuer.to_string(), keys.jwk_set));

        // exp well in the past (beyond default 60s leeway)
        let claims = make_claims(issuer, audience, "sub", -120);
        let token = encode_jwt(&claims, &keys.encoding_key, &keys.kid);

        let result = validate(config, cache, &token).await;
        assert_eq!(result, Err(StatusCode::UNAUTHORIZED));
    }

    #[tokio::test]
    async fn wrong_audience_fails() {
        let keys = generate_test_keys();
        let issuer = "https://hydra.example.com";
        let config = make_config(issuer, "emumet");
        let cache = Arc::new(JwksCache::new_with_jwks(issuer.to_string(), keys.jwk_set));

        let claims = make_claims(issuer, "wrong-audience", "sub", 3600);
        let token = encode_jwt(&claims, &keys.encoding_key, &keys.kid);

        let result = validate(config, cache, &token).await;
        assert_eq!(result, Err(StatusCode::UNAUTHORIZED));
    }

    #[tokio::test]
    async fn wrong_issuer_fails() {
        let keys = generate_test_keys();
        let issuer = "https://hydra.example.com";
        let audience = "emumet";
        let config = make_config(issuer, audience);
        let cache = Arc::new(JwksCache::new_with_jwks(issuer.to_string(), keys.jwk_set));

        let claims = make_claims("https://evil-issuer.example.com", audience, "sub", 3600);
        let token = encode_jwt(&claims, &keys.encoding_key, &keys.kid);

        let result = validate(config, cache, &token).await;
        assert_eq!(result, Err(StatusCode::UNAUTHORIZED));
    }

    #[tokio::test]
    async fn missing_authorization_header_fails() {
        let keys = generate_test_keys();
        let issuer = "https://hydra.example.com";
        let config = make_config(issuer, "emumet");
        let cache = Arc::new(JwksCache::new_with_jwks(issuer.to_string(), keys.jwk_set));

        let mut req: Request<Body> = Request::builder().body(Body::empty()).unwrap();
        let result = auth_middleware_core(config, cache, &mut req).await;
        assert_eq!(result, Err(StatusCode::UNAUTHORIZED));
    }

    #[tokio::test]
    async fn wrong_signing_key_fails() {
        let keys = generate_test_keys();
        let wrong_keys = generate_test_keys(); // different RSA key pair
        let issuer = "https://hydra.example.com";
        let audience = "emumet";
        let config = make_config(issuer, audience);
        // Cache has `keys.jwk_set`, but token is signed with `wrong_keys`
        let cache = Arc::new(JwksCache::new_with_jwks(issuer.to_string(), keys.jwk_set));

        let claims = make_claims(issuer, audience, "sub", 3600);
        // Sign with wrong key but use the kid from the cached keyset
        let token = encode_jwt(&claims, &wrong_keys.encoding_key, &keys.kid);

        let result = validate(config, cache, &token).await;
        assert_eq!(result, Err(StatusCode::UNAUTHORIZED));
    }

    #[tokio::test]
    async fn oidc_auth_info_from_claims() {
        let claims = AuthClaims {
            iss: "https://hydra.example.com".to_string(),
            sub: "kratos-uuid-abc".to_string(),
            aud: OneOrMany::One("emumet".to_string()),
            exp: unix_now() + 3600,
        };
        let info: OidcAuthInfo = claims.into();
        assert_eq!(info.issuer, "https://hydra.example.com");
        assert_eq!(info.subject, "kratos-uuid-abc");
    }

    #[test]
    fn one_or_many_variants() {
        let one = OneOrMany::One("emumet".to_string());
        assert!(one.contains("emumet"));
        assert!(!one.contains("other"));

        let many = OneOrMany::Many(vec!["emumet".to_string(), "other".to_string()]);
        assert!(many.contains("emumet"));
        assert!(many.contains("other"));
        assert!(!many.contains("missing"));
    }
}
