use std::collections::HashMap;

use base64::{engine::general_purpose, Engine as _};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CavageSignature {
    pub(crate) key_id: String,
    pub(crate) algorithm: String,
    pub(crate) headers: Vec<String>,
    pub(crate) signature: Vec<u8>,
}

pub(crate) fn parse_cavage_signature(header: &str) -> std::result::Result<CavageSignature, String> {
    let mut params = HashMap::new();
    for part in split_signature_params(header)? {
        let (name, value) = parse_signature_param(&part)?;
        if params.insert(name.to_ascii_lowercase(), value).is_some() {
            return Err("duplicate signature parameter is not allowed".to_string());
        }
    }

    let key_id = params
        .remove("keyid")
        .ok_or_else(|| "keyId parameter is required".to_string())?;
    let algorithm = params
        .remove("algorithm")
        .unwrap_or_else(|| "rsa-sha256".to_string());
    let headers = params
        .remove("headers")
        .map(|value| {
            value
                .split_whitespace()
                .map(|header| header.to_ascii_lowercase())
                .collect::<Vec<_>>()
        })
        .filter(|headers| !headers.is_empty())
        .unwrap_or_else(|| vec!["date".to_string()]);
    let signature = params
        .remove("signature")
        .ok_or_else(|| "signature parameter is required".to_string())?;
    let signature = general_purpose::STANDARD
        .decode(signature.as_bytes())
        .map_err(|e| format!("signature is not valid base64: {e}"))?;

    Ok(CavageSignature {
        key_id,
        algorithm,
        headers,
        signature,
    })
}

fn split_signature_params(header: &str) -> std::result::Result<Vec<String>, String> {
    let mut params = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut escaped = false;

    for ch in header.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }

        match ch {
            '\\' if in_quotes => {
                current.push(ch);
                escaped = true;
            }
            '"' => {
                in_quotes = !in_quotes;
                current.push(ch);
            }
            ',' if !in_quotes => {
                if !current.trim().is_empty() {
                    params.push(current.trim().to_string());
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }

    if in_quotes {
        return Err("unterminated quoted parameter".to_string());
    }

    if !current.trim().is_empty() {
        params.push(current.trim().to_string());
    }

    if params.is_empty() {
        return Err("Signature header is empty".to_string());
    }

    Ok(params)
}

fn parse_signature_param(part: &str) -> std::result::Result<(String, String), String> {
    let (name, value) = part
        .split_once('=')
        .ok_or_else(|| format!("parameter is missing '=': {part}"))?;
    let name = name.trim();
    if name.is_empty() {
        return Err("parameter name is empty".to_string());
    }

    let value = value.trim();
    let value = if value.starts_with('"') {
        parse_quoted_value(value)?
    } else {
        value.to_string()
    };

    Ok((name.to_string(), value))
}

fn parse_quoted_value(value: &str) -> std::result::Result<String, String> {
    if !value.ends_with('"') || value.len() < 2 {
        return Err("quoted parameter is not closed".to_string());
    }

    let inner = &value[1..value.len() - 1];
    let mut output = String::new();
    let mut escaped = false;

    for ch in inner.chars() {
        if escaped {
            output.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else {
            output.push(ch);
        }
    }

    if escaped {
        return Err("quoted parameter ends with an escape".to_string());
    }

    Ok(output)
}
