//! Webhook 署名 (HMAC-SHA256)。
//!
//! 署名対象は `"{timestamp}." + 本文` (timestamp は Unix 秒の 10 進文字列)。署名ヘッダーは
//! `v1=<小文字 hex>`。timestamp を署名対象に含めることで、受信側は許容時刻差を超えた再送
//! (リプレイ) を拒否できる。検証は定数時間比較 (`Mac::verify_slice`)。

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

pub const SIGNATURE_VERSION: &str = "v1";

fn mac(secret: &[u8], timestamp: i64, body: &[u8]) -> HmacSha256 {
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts keys of any length");
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(body);
    mac
}

fn to_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

fn from_hex(text: &str) -> Option<Vec<u8>> {
    if text.is_empty() || !text.len().is_multiple_of(2) {
        return None;
    }
    text.as_bytes()
        .chunks(2)
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16)?;
            let low = (pair[1] as char).to_digit(16)?;
            Some((high * 16 + low) as u8)
        })
        .collect()
}

/// 署名ヘッダー値 (`v1=<hex>`) を作る。
pub fn sign(secret: &[u8], timestamp: i64, body: &[u8]) -> String {
    let digest = mac(secret, timestamp, body).finalize().into_bytes();
    format!("{SIGNATURE_VERSION}={}", to_hex(&digest))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureError {
    /// `v1=<hex>` の形式でない。
    Malformed,
    /// timestamp が許容時刻差を超えている (リプレイ緩和)。
    TimestampOutOfTolerance,
    Mismatch,
}

/// 受信側の検証手順の参照実装 (テストと文書のテストベクタで使う)。
///
/// 1. timestamp ヘッダーを整数として解釈し、現在時刻との差が `tolerance_secs` 以内か確認する
/// 2. `"{timestamp}." + 受信した本文のバイト列` の HMAC-SHA256 を計算する
/// 3. 署名ヘッダー `v1=<hex>` と定数時間で比較する
pub fn verify(
    secret: &[u8],
    signature_header: &str,
    timestamp: i64,
    body: &[u8],
    now_unix: i64,
    tolerance_secs: i64,
) -> Result<(), SignatureError> {
    let hex = signature_header
        .strip_prefix(SIGNATURE_VERSION)
        .and_then(|rest| rest.strip_prefix('='))
        .ok_or(SignatureError::Malformed)?;
    let expected = from_hex(hex).ok_or(SignatureError::Malformed)?;
    if (now_unix - timestamp).abs() > tolerance_secs {
        return Err(SignatureError::TimestampOutOfTolerance);
    }
    mac(secret, timestamp, body)
        .verify_slice(&expected)
        .map_err(|_| SignatureError::Mismatch)
}
