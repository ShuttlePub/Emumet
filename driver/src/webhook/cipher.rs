//! 送信先 secret の暗号化 (AES-256-GCM)。
//!
//! 自前実装の暗号は使わず、実績のある AEAD (`aes-gcm` crate。署名鍵の暗号化と同じ依存) を使う。
//! - 鍵はサーバー設定 (環境変数 `REGISTRATION_WEBHOOK_ENCRYPTION_KEY` の base64 32 バイト、または
//!   `REGISTRATION_WEBHOOK_ENCRYPTION_KEY_FILE` のファイル)。DB・ログ・応答に出さない。
//! - 暗号文 = `nonce(12 バイト) || ciphertext+tag`。関連データ (AAD) に送信先 ID を束縛するので、
//!   別の送信先の行に暗号文を移しても復号できない。
//! - 鍵識別子 (`kf-` + 鍵の SHA-256 の先頭 4 バイト hex) を暗号文に添える。鍵を変えた・誤った場合は
//!   識別子の不一致で復号不可になる (将来の鍵ローテーションの足がかり。ローテーション機構自体は範囲外)。

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use kernel::interfaces::repository::{WebhookSecretCipher, WebhookSecretError};
use kernel::prelude::entity::{SealedWebhookSecret, WebhookEndpointId, WebhookSecret};
use rand::rngs::OsRng;
use rand::RngCore;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

pub const ENV_ENCRYPTION_KEY: &str = "REGISTRATION_WEBHOOK_ENCRYPTION_KEY";
pub const ENV_ENCRYPTION_KEY_FILE: &str = "REGISTRATION_WEBHOOK_ENCRYPTION_KEY_FILE";

const KEY_BYTES: usize = 32;
const NONCE_BYTES: usize = 12;
const AAD_PREFIX: &str = "emumet:registration-gate-webhook:secret:v1:";

#[derive(Clone)]
struct KeyMaterial {
    key: Zeroizing<[u8; KEY_BYTES]>,
    key_id: String,
}

/// 暗号鍵の設定状態。値は含まない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncryptionKeyStatus {
    /// 鍵が設定されている。
    Configured,
    /// 鍵が未設定 (送信先の作成・secret 更新は拒否される)。
    NotConfigured,
    /// 鍵の指定が不正 (直接指定とファイル指定の併用・読めない・base64 でない・32 バイトでない)。
    /// 未設定と同じく扱う。
    Invalid,
}

#[derive(Clone)]
pub struct AeadWebhookSecretCipher {
    key: Option<KeyMaterial>,
    status: EncryptionKeyStatus,
}

impl std::fmt::Debug for AeadWebhookSecretCipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AeadWebhookSecretCipher")
            .field("status", &self.status)
            .finish()
    }
}

fn key_id_of(key: &[u8; KEY_BYTES]) -> String {
    let digest = Sha256::digest(key);
    let mut id = String::from("kf-");
    for byte in &digest[..4] {
        id.push_str(&format!("{byte:02x}"));
    }
    id
}

fn parse_key(encoded: &str) -> Option<Zeroizing<[u8; KEY_BYTES]>> {
    let decoded = Zeroizing::new(BASE64.decode(encoded.trim()).ok()?);
    let bytes: [u8; KEY_BYTES] = decoded.as_slice().try_into().ok()?;
    Some(Zeroizing::new(bytes))
}

impl AeadWebhookSecretCipher {
    /// 鍵なし (送信先の作成・secret 更新は拒否される)。
    pub fn not_configured() -> Self {
        Self {
            key: None,
            status: EncryptionKeyStatus::NotConfigured,
        }
    }

    /// 生の 32 バイト鍵から構築する (検証用・テスト用)。
    pub fn from_key_bytes(key: [u8; KEY_BYTES]) -> Self {
        let key = Zeroizing::new(key);
        let key_id = key_id_of(&key);
        Self {
            key: Some(KeyMaterial { key, key_id }),
            status: EncryptionKeyStatus::Configured,
        }
    }

    /// サーバー設定 (環境変数・secret ファイル) から構築する。不備は起動失敗にせず `Invalid`
    /// (未設定と同じ扱い) になるだけで、値はログに出さない。
    pub fn from_env() -> Self {
        let direct = dotenvy::var(ENV_ENCRYPTION_KEY).ok();
        let file = dotenvy::var(ENV_ENCRYPTION_KEY_FILE).ok();
        Self::from_values(direct.as_deref(), file.as_deref())
    }

    pub fn from_values(direct: Option<&str>, file: Option<&str>) -> Self {
        let direct = direct.filter(|value| !value.trim().is_empty());
        let file = file.filter(|value| !value.trim().is_empty());
        let encoded = match (direct, file) {
            (None, None) => return Self::not_configured(),
            (Some(_), Some(_)) => return Self::invalid(),
            (Some(value), None) => Zeroizing::new(value.to_string()),
            (None, Some(path)) => match std::fs::read_to_string(path) {
                Ok(content) => Zeroizing::new(content),
                Err(_) => return Self::invalid(),
            },
        };
        match parse_key(&encoded) {
            Some(key) => Self::from_key_bytes(*key),
            None => Self::invalid(),
        }
    }

    fn invalid() -> Self {
        Self {
            key: None,
            status: EncryptionKeyStatus::Invalid,
        }
    }

    pub fn key_status(&self) -> EncryptionKeyStatus {
        self.status
    }

    /// 現在の鍵識別子 (鍵そのものではない)。
    pub fn key_id(&self) -> Option<&str> {
        self.key.as_ref().map(|key| key.key_id.as_str())
    }
}

fn aad(endpoint_id: WebhookEndpointId) -> Vec<u8> {
    format!("{AAD_PREFIX}{}", endpoint_id.as_i64()).into_bytes()
}

impl WebhookSecretCipher for AeadWebhookSecretCipher {
    fn is_configured(&self) -> bool {
        self.key.is_some()
    }

    fn seal(
        &self,
        endpoint_id: WebhookEndpointId,
        secret: &WebhookSecret,
    ) -> Result<SealedWebhookSecret, WebhookSecretError> {
        let key = self
            .key
            .as_ref()
            .ok_or(WebhookSecretError::KeyNotConfigured)?;
        let cipher = Aes256Gcm::new_from_slice(key.key.as_slice())
            .map_err(|_| WebhookSecretError::KeyNotConfigured)?;
        let mut nonce_bytes = [0u8; NONCE_BYTES];
        OsRng.fill_bytes(&mut nonce_bytes);
        let encrypted = cipher
            .encrypt(
                Nonce::from_slice(&nonce_bytes),
                Payload {
                    msg: secret.expose_bytes(),
                    aad: &aad(endpoint_id),
                },
            )
            .map_err(|_| WebhookSecretError::Undecryptable)?;
        let mut ciphertext = Vec::with_capacity(NONCE_BYTES + encrypted.len());
        ciphertext.extend_from_slice(&nonce_bytes);
        ciphertext.extend_from_slice(&encrypted);
        Ok(SealedWebhookSecret {
            key_id: key.key_id.clone(),
            ciphertext,
        })
    }

    fn open(
        &self,
        endpoint_id: WebhookEndpointId,
        sealed: &SealedWebhookSecret,
    ) -> Result<WebhookSecret, WebhookSecretError> {
        let key = self
            .key
            .as_ref()
            .ok_or(WebhookSecretError::KeyNotConfigured)?;
        if sealed.key_id != key.key_id || sealed.ciphertext.len() <= NONCE_BYTES {
            return Err(WebhookSecretError::Undecryptable);
        }
        let (nonce_bytes, encrypted) = sealed.ciphertext.split_at(NONCE_BYTES);
        let cipher = Aes256Gcm::new_from_slice(key.key.as_slice())
            .map_err(|_| WebhookSecretError::KeyNotConfigured)?;
        let plaintext = cipher
            .decrypt(
                Nonce::from_slice(nonce_bytes),
                Payload {
                    msg: encrypted,
                    aad: &aad(endpoint_id),
                },
            )
            .map(Zeroizing::new)
            .map_err(|_| WebhookSecretError::Undecryptable)?;
        WebhookSecret::from_trusted_bytes(plaintext.to_vec())
            .map_err(|_| WebhookSecretError::Undecryptable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_A: [u8; 32] = [7u8; 32];
    const KEY_B: [u8; 32] = [9u8; 32];

    fn secret() -> WebhookSecret {
        WebhookSecret::parse("receiver-secret-0123456789abcdef").unwrap()
    }

    #[test]
    fn round_trip_and_ciphertext_does_not_contain_the_plaintext() {
        let cipher = AeadWebhookSecretCipher::from_key_bytes(KEY_A);
        let id = WebhookEndpointId::new(42);
        let sealed = cipher.seal(id, &secret()).unwrap();
        assert!(!sealed
            .ciphertext
            .windows(secret().expose_bytes().len())
            .any(|window| window == secret().expose_bytes()));
        assert!(sealed.key_id.starts_with("kf-"));
        let opened = cipher.open(id, &sealed).unwrap();
        assert_eq!(opened.expose_bytes(), secret().expose_bytes());
    }

    #[test]
    fn nonce_is_random_per_seal() {
        let cipher = AeadWebhookSecretCipher::from_key_bytes(KEY_A);
        let id = WebhookEndpointId::new(1);
        let first = cipher.seal(id, &secret()).unwrap();
        let second = cipher.seal(id, &secret()).unwrap();
        assert_ne!(first.ciphertext, second.ciphertext);
    }

    #[test]
    fn ciphertext_is_bound_to_the_endpoint_id() {
        let cipher = AeadWebhookSecretCipher::from_key_bytes(KEY_A);
        let sealed = cipher.seal(WebhookEndpointId::new(1), &secret()).unwrap();
        assert_eq!(
            cipher.open(WebhookEndpointId::new(2), &sealed).unwrap_err(),
            WebhookSecretError::Undecryptable
        );
    }

    #[test]
    fn a_different_key_cannot_decrypt() {
        let sealed = AeadWebhookSecretCipher::from_key_bytes(KEY_A)
            .seal(WebhookEndpointId::new(1), &secret())
            .unwrap();
        let other = AeadWebhookSecretCipher::from_key_bytes(KEY_B);
        assert_eq!(
            other.open(WebhookEndpointId::new(1), &sealed).unwrap_err(),
            WebhookSecretError::Undecryptable
        );
        assert_ne!(
            AeadWebhookSecretCipher::from_key_bytes(KEY_A).key_id(),
            other.key_id()
        );
    }

    #[test]
    fn tampered_or_truncated_ciphertext_is_undecryptable() {
        let cipher = AeadWebhookSecretCipher::from_key_bytes(KEY_A);
        let id = WebhookEndpointId::new(1);
        let mut sealed = cipher.seal(id, &secret()).unwrap();
        let last = sealed.ciphertext.len() - 1;
        sealed.ciphertext[last] ^= 0x01;
        assert_eq!(
            cipher.open(id, &sealed).unwrap_err(),
            WebhookSecretError::Undecryptable
        );
        sealed.ciphertext.truncate(5);
        assert_eq!(
            cipher.open(id, &sealed).unwrap_err(),
            WebhookSecretError::Undecryptable
        );
    }

    #[test]
    fn without_a_key_nothing_is_sealed_or_opened() {
        let cipher = AeadWebhookSecretCipher::not_configured();
        assert!(!cipher.is_configured());
        assert_eq!(
            cipher
                .seal(WebhookEndpointId::new(1), &secret())
                .unwrap_err(),
            WebhookSecretError::KeyNotConfigured
        );
        let sealed = AeadWebhookSecretCipher::from_key_bytes(KEY_A)
            .seal(WebhookEndpointId::new(1), &secret())
            .unwrap();
        assert_eq!(
            cipher.open(WebhookEndpointId::new(1), &sealed).unwrap_err(),
            WebhookSecretError::KeyNotConfigured
        );
    }

    #[test]
    fn key_configuration_is_parsed_strictly_and_never_printed() {
        let good = BASE64.encode(KEY_A);
        assert_eq!(
            AeadWebhookSecretCipher::from_values(Some(&good), None).key_status(),
            EncryptionKeyStatus::Configured
        );
        assert_eq!(
            AeadWebhookSecretCipher::from_values(None, None).key_status(),
            EncryptionKeyStatus::NotConfigured
        );
        assert_eq!(
            AeadWebhookSecretCipher::from_values(Some("  "), None).key_status(),
            EncryptionKeyStatus::NotConfigured
        );
        for bad in [
            "not-base64!!",
            &BASE64.encode([1u8; 16]),
            &BASE64.encode([1u8; 33]),
        ] {
            assert_eq!(
                AeadWebhookSecretCipher::from_values(Some(bad), None).key_status(),
                EncryptionKeyStatus::Invalid,
                "{bad}"
            );
        }
        assert_eq!(
            AeadWebhookSecretCipher::from_values(Some(&good), Some("/nonexistent")).key_status(),
            EncryptionKeyStatus::Invalid
        );
        assert_eq!(
            AeadWebhookSecretCipher::from_values(None, Some("/nonexistent/key")).key_status(),
            EncryptionKeyStatus::Invalid
        );
        let rendered = format!(
            "{:?}",
            AeadWebhookSecretCipher::from_values(Some(&good), None)
        );
        assert!(!rendered.contains(&good));
    }

    #[test]
    fn key_can_be_loaded_from_a_file() {
        let dir = std::env::temp_dir().join(format!("emumet-webhook-key-{}", crate_unique()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("key");
        std::fs::write(&path, format!("{}\n", BASE64.encode(KEY_A))).unwrap();
        let cipher = AeadWebhookSecretCipher::from_values(None, path.to_str());
        assert_eq!(cipher.key_status(), EncryptionKeyStatus::Configured);
        assert_eq!(
            cipher.key_id(),
            AeadWebhookSecretCipher::from_key_bytes(KEY_A).key_id()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    fn crate_unique() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }
}
