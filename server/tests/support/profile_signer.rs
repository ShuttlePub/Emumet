//! Test-only signing with a Profile's own key.
//!
//! The server has no signing API: a Profile key is only used by the delivery worker. A scenario
//! that needs a Profile-signed request the worker does not produce (the Create/Note a later
//! ShuttlePub release will post) therefore reads the encrypted key from the database and
//! decrypts it with the master key password, exactly as the delivery worker does. This fixture
//! never talks to the server and needs the same password file the server uses.

use super::db;
use driver::crypto::{Argon2Encryptor, FilePasswordProvider};
use driver::http_signing::HttpSignerImpl;
use kernel::interfaces::crypto::{
    EncryptedPrivateKey, KeyEncryptor, PasswordProvider, SigningAlgorithm,
};
use kernel::interfaces::http_signing::{HttpSigner, HttpSigningRequest};
use std::collections::HashMap;
use std::path::PathBuf;

fn password_provider() -> FilePasswordProvider {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("server crate is under the workspace root")
        .join("master-key-password");
    FilePasswordProvider::with_paths("/run/secrets/master-key-password", root)
}

/// Signs `request` as the Profile and returns the headers to add to the real request
/// (`Signature` and the signed header values).
pub async fn sign_as_profile(
    profile_nanoid: &str,
    request: &HttpSigningRequest,
) -> HashMap<String, String> {
    let (key_id, encrypted, _public_pem) = db::active_profile_key(profile_nanoid).await;
    let encrypted: EncryptedPrivateKey =
        serde_json::from_value(encrypted).expect("stored encrypted key");
    let password = password_provider()
        .get_password()
        .expect("master key password file");
    let private_key = Argon2Encryptor::default()
        .decrypt(&encrypted, &password)
        .expect("decrypt the Profile key");
    HttpSignerImpl
        .sign(request, &private_key, &key_id, &SigningAlgorithm::Rsa2048)
        .await
        .expect("sign as Profile")
        .cavage_headers
}
