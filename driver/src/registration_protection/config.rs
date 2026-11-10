use kernel::prelude::entity::ConfigGeneration;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use zeroize::Zeroize;

/// 設定値の取得元。環境変数 (`ProcessEnv`) と、テスト用のマップ (`EnvSource`) を差し替えられる。
pub trait ValueSource {
    fn get(&self, key: &str) -> Option<String>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ProcessEnv;

impl ValueSource for ProcessEnv {
    fn get(&self, key: &str) -> Option<String> {
        dotenvy::var(key).ok()
    }
}

#[derive(Debug, Default, Clone)]
pub struct EnvSource(HashMap<String, String>);

impl EnvSource {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, key: &str, value: &str) -> Self {
        self.0.insert(key.to_string(), value.to_string());
        self
    }
}

impl ValueSource for EnvSource {
    fn get(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
}

/// 空・空白だけの値は未設定として扱う。
pub(crate) fn non_empty(source: &dyn ValueSource, key: &str) -> Option<String> {
    source
        .get(key)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// 明示無効 (`false`/`0`/`off`/`no`/`disabled`)。`true` 系と未設定は None。
pub(crate) fn explicit_enabled(source: &dyn ValueSource, key: &str) -> Option<bool> {
    non_empty(source, key).map(|value| {
        !matches!(
            value.to_ascii_lowercase().as_str(),
            "false" | "0" | "off" | "no" | "disabled"
        )
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SecretError {
    Ambiguous,
    Unreadable,
}

pub(crate) struct Secret(Vec<u8>);

impl Secret {
    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub(crate) fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0).unwrap_or("")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// 直接指定とファイル指定の secret を読む。両方指定は曖昧で不備、未指定は `Ok(None)`。
pub(crate) fn load_secret(
    source: &dyn ValueSource,
    direct_key: &str,
    file_key: &str,
) -> Result<Option<Secret>, SecretError> {
    let direct = non_empty(source, direct_key);
    let file = non_empty(source, file_key);
    match (direct, file) {
        (Some(_), Some(_)) => Err(SecretError::Ambiguous),
        (None, None) => Ok(None),
        (Some(value), None) => Ok(Some(Secret(value.into_bytes()))),
        (None, Some(path)) => {
            let mut content = std::fs::read_to_string(path).map_err(|_| SecretError::Unreadable)?;
            let trimmed = content.trim().as_bytes().to_vec();
            content.zeroize();
            if trimmed.is_empty() {
                return Err(SecretError::Unreadable);
            }
            Ok(Some(Secret(trimmed)))
        }
    }
}

/// secret の逆算できない短い fingerprint (非 secret)。ドメイン分離した SHA-256 の先頭 48 bit。
/// 弱い secret は総当たりで照合され得るので、十分長いランダム値を使うこと (docs 参照)。
pub fn secret_fingerprint(secret: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"emumet:registration-protection:secret-fingerprint:v1\0");
    hasher.update(secret);
    hex(&hasher.finalize()[..6])
}

/// 設定世代: 保護・provider・enabled・非 secret の設定値・secret fingerprint から決定的に導く。
/// フィールドは長さ接頭辞つきで連結するため、値の境界が曖昧にならない。
pub fn derive_generation(
    protection: &str,
    provider: &str,
    enabled: bool,
    fields: &[(&str, &str)],
    secret_fingerprint: &str,
) -> ConfigGeneration {
    let mut hasher = Sha256::new();
    hasher.update(b"emumet:registration-protection:generation:v1\0");
    let mut push = |value: &str| {
        hasher.update((value.len() as u64).to_be_bytes());
        hasher.update(value.as_bytes());
    };
    push(protection);
    push(provider);
    push(if enabled { "enabled" } else { "disabled" });
    for (key, value) in fields {
        push(key);
        push(value);
    }
    push(secret_fingerprint);
    ConfigGeneration::new(hex(&hasher.finalize()[..16]))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
