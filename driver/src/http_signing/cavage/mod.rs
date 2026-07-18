pub(super) mod parser;

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use base64::{engine::general_purpose, Engine as _};
use kernel::interfaces::crypto::SignatureVerifier as CryptoSignatureVerifier;
use kernel::interfaces::http_signing::HttpSignatureVerificationInput;
use sha2::Digest;

pub(super) fn cavage_signing_string(
    request: &HttpSignatureVerificationInput,
    headers: &[String],
) -> std::result::Result<Vec<u8>, String> {
    let url = reqwest::Url::parse(&request.url)
        .map_err(|e| format!("MalformedSignature: request URL is invalid: {e}"))?;
    let path_and_query = match url.query() {
        Some(query) => format!("{}?{}", url.path(), query),
        None => url.path().to_string(),
    };

    let mut lines = Vec::with_capacity(headers.len());
    for header in headers {
        if header == "(request-target)" {
            lines.push(format!(
                "(request-target): {} {}",
                request.method.to_ascii_lowercase(),
                path_and_query
            ));
            continue;
        }

        let value = get_header(&request.headers, header).ok_or_else(|| {
            format!("MalformedSignature: signed header '{header}' is missing from request")
        })?;
        lines.push(format!("{}: {}", header.to_ascii_lowercase(), value));
    }

    Ok(lines.join("\n").into_bytes())
}

pub(super) fn verify_cavage_signature(
    signing_string: &[u8],
    signature: &[u8],
    public_key_pem: &[u8],
    algorithm: &str,
) -> std::result::Result<bool, String> {
    let algorithm = algorithm.to_ascii_lowercase();

    if algorithm == "ed25519" || algorithm == "ed25519-sha256" {
        return crate::crypto::Ed25519Verifier
            .verify(signing_string, signature, public_key_pem)
            .map_err(|e| format!("InvalidSignature: Ed25519 verification failed: {e:?}"));
    }

    if algorithm == "rsa-sha256" || algorithm == "hs2019" {
        match crate::crypto::Rsa2048Verifier.verify(signing_string, signature, public_key_pem) {
            Ok(valid) => return Ok(valid),
            Err(rsa_error) if algorithm == "hs2019" => {
                return crate::crypto::Ed25519Verifier
                    .verify(signing_string, signature, public_key_pem)
                    .map_err(|ed_error| {
                        format!(
                            "InvalidSignature: hs2019 verification failed: RSA {rsa_error:?}; Ed25519 {ed_error:?}"
                        )
                    });
            }
            Err(e) => {
                return Err(format!("InvalidSignature: RSA verification failed: {e:?}"));
            }
        }
    }

    Err(format!(
        "MalformedSignature: unsupported signature algorithm '{algorithm}'"
    ))
}

pub(super) fn validate_date_header(
    request: &HttpSignatureVerificationInput,
    tolerance: Duration,
) -> std::result::Result<(), String> {
    let date = get_header(&request.headers, "date")
        .ok_or_else(|| "StaleDate: Date header is required".to_string())?;
    let parsed = httpdate::parse_http_date(date)
        .map_err(|e| format!("StaleDate: Date header is malformed: {e}"))?;
    let now = SystemTime::now();

    if parsed > now {
        let delta = parsed
            .duration_since(now)
            .unwrap_or_else(|_| Duration::from_secs(0));
        if delta > tolerance {
            return Err("StaleDate: Date header is too far in the future".to_string());
        }
    } else {
        let delta = now
            .duration_since(parsed)
            .unwrap_or_else(|_| Duration::from_secs(0));
        if delta > tolerance {
            return Err("StaleDate: Date header is expired".to_string());
        }
    }

    Ok(())
}

pub(super) fn validate_digest_header(
    request: &HttpSignatureVerificationInput,
) -> std::result::Result<(), String> {
    let has_body = request.body.as_ref().is_some_and(|body| !body.is_empty());
    let Some(digest_header) = get_header(&request.headers, "digest") else {
        if has_body {
            return Err(
                "DigestMismatch: Digest header is required for requests with a body".to_string(),
            );
        }
        return Ok(());
    };
    let expected = digest_header
        .split(',')
        .find_map(|part| {
            let (name, value) = part.trim().split_once('=')?;
            if name.eq_ignore_ascii_case("sha-256") {
                Some(value.trim())
            } else {
                None
            }
        })
        .ok_or_else(|| "DigestMismatch: Digest header is missing SHA-256 value".to_string())?;

    let expected = general_purpose::STANDARD
        .decode(expected.as_bytes())
        .map_err(|e| format!("DigestMismatch: SHA-256 digest is not valid base64: {e}"))?;
    let actual = sha2::Sha256::digest(request.body.as_deref().unwrap_or_default());

    if expected.as_slice() == actual.as_slice() {
        Ok(())
    } else {
        Err("DigestMismatch: body digest does not match Digest header".to_string())
    }
}

pub(super) fn validate_required_signed_headers(
    request: &HttpSignatureVerificationInput,
    signed_headers: &[String],
) -> std::result::Result<(), String> {
    for required in ["(request-target)", "host", "date"] {
        if !signed_headers.iter().any(|header| header == required) {
            return Err(format!(
                "MalformedSignature: required header '{required}' must be signed"
            ));
        }
    }

    let has_body = request.body.as_ref().is_some_and(|body| !body.is_empty());
    if (has_body || get_header(&request.headers, "digest").is_some())
        && !signed_headers.iter().any(|header| header == "digest")
    {
        return Err(
            "MalformedSignature: Digest header must be signed when a body is present".to_string(),
        );
    }

    Ok(())
}

pub(super) fn get_header<'a>(headers: &'a HashMap<String, String>, name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}
