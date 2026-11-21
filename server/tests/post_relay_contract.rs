//! Contract test for `docs/contracts/post-relay-v1*` (execution unit `emumet-p1-relay-contract`, C0).
//!
//! It checks, without a database, an Ory stack or the network:
//!
//! * the snapshot is what it claims to be (host commit, version, checksums of every file);
//! * `v1.md`, `v1.schema.json`, `v1.constants.json` and the fixtures agree on the bounded constants;
//! * the schema is valid, accepts the P1 envelopes and rejects the malformed ones;
//! * a small reference receiver that implements `v1.md` section 7 with **no link ledger** reproduces
//!   every P1 fixture, and also accepts the P2 envelopes (so no P2 rule is hidden in P1);
//! * the outer-signature vectors, the retransmission invariants and the original-signature evidence;
//! * a small reference reader that implements `v1.md` section 8 reproduces every inbox fixture, and
//!   every value it forwards is accepted by the envelope schema;
//! * the field-format fixtures (timestamps, URLs) agree with the schema and with `v1.md` section 4.5;
//! * a reference implementation of the header-evidence algorithm (`v1.md` section 4.3) reproduces the
//!   input/output fixtures.
//!
//! The reference code lives only in this test. It is not the production implementation.

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use rsa::pkcs8::DecodePublicKey;
use rsa::{Pkcs1v15Sign, RsaPublicKey};
use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

fn contracts_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/contracts")
}

fn fixtures_dir() -> PathBuf {
    contracts_dir().join("post-relay-v1-fixtures")
}

fn read_text(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&read_text(path))
        .unwrap_or_else(|e| panic!("invalid JSON in {}: {e}", path.display()))
}

fn json_files_in(dir: &Path) -> Vec<(String, Value)> {
    let mut names: Vec<_> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot list {}: {e}", dir.display()))
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.ends_with(".json"))
        .collect();
    names.sort();
    names
        .into_iter()
        .map(|n| {
            let v = read_json(&dir.join(&n));
            (n, v)
        })
        .collect()
}

fn constants() -> Value {
    read_json(&contracts_dir().join("post-relay-v1.constants.json"))["constants"].clone()
}

fn constant(name: &str) -> u64 {
    constants()[name]
        .as_u64()
        .unwrap_or_else(|| panic!("constant {name} missing"))
}

fn schema() -> Value {
    read_json(&contracts_dir().join("post-relay-v1.schema.json"))
}

fn validator() -> jsonschema::Validator {
    jsonschema::validator_for(&schema()).expect("the envelope schema must compile")
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key]
        .as_str()
        .unwrap_or_else(|| panic!("string field {key} missing in {v}"))
}

// ---------------------------------------------------------------------------------------------
// snapshot
// ---------------------------------------------------------------------------------------------

#[test]
fn snapshot_matches_its_manifest() {
    let dir = contracts_dir();
    let snap = read_json(&dir.join("post-relay-v1.snapshot.json"));
    assert_eq!(snap["host_repo"], "ShuttlePub/host");
    assert_eq!(snap["contract"], "emumet-post-relay");
    assert_eq!(snap["version"], "v1");
    assert_eq!(
        snap["revision"],
        read_json(&dir.join("post-relay-v1.constants.json"))["revision"]
    );
    let commit = s(&snap, "host_commit");
    assert!(
        commit.len() == 40 && commit.chars().all(|c| c.is_ascii_hexdigit()),
        "host_commit must be a full 40-hex commit id, got {commit:?}"
    );

    let mut listed = BTreeSet::new();
    for f in snap["files"].as_array().unwrap() {
        let rel = s(f, "path");
        let bytes = fs::read(dir.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
        assert_eq!(sha256_hex(&bytes), s(f, "sha256"), "checksum of {rel}");
        assert!(
            s(f, "host_path").starts_with("intents/shuttlepub/contracts/emumet-post-relay/"),
            "host_path of {rel}"
        );
        listed.insert(rel.to_string());
    }

    // Every file of the snapshot is listed: nothing was added next to the copy.
    let mut present = BTreeSet::new();
    fn walk(base: &Path, dir: &Path, out: &mut BTreeSet<String>) {
        for e in fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(base, &p, out);
            } else {
                out.insert(p.strip_prefix(base).unwrap().to_string_lossy().into_owned());
            }
        }
    }
    walk(&dir, &dir, &mut present);
    present.remove("post-relay-v1.snapshot.json");
    present.remove("README.md");
    assert_eq!(
        present, listed,
        "files in docs/contracts and in the manifest differ"
    );
}

// ---------------------------------------------------------------------------------------------
// constants
// ---------------------------------------------------------------------------------------------

#[test]
fn document_table_matches_constants_file() {
    let doc = read_text(&contracts_dir().join("post-relay-v1.md"));
    let consts = constants();
    let mut seen = BTreeSet::new();
    for line in doc.lines() {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        // | `NAME` | 123 | meaning |
        if cells.len() < 4 || !cells[1].starts_with('`') || !cells[1].ends_with('`') {
            continue;
        }
        let name = cells[1].trim_matches('`');
        let Ok(value) = cells[2].parse::<u64>() else {
            continue;
        };
        if let Some(expected) = consts.get(name) {
            assert_eq!(
                expected.as_u64(),
                Some(value),
                "v1.md table value of {name}"
            );
            seen.insert(name.to_string());
        }
    }
    let all: BTreeSet<String> = consts.as_object().unwrap().keys().cloned().collect();
    assert_eq!(
        seen, all,
        "every constant must appear once in the v1.md table"
    );
}

#[test]
fn schema_bounds_match_constants() {
    let sch = schema();
    let c = constant;
    let at = |ptr: &str| -> u64 {
        sch.pointer(ptr)
            .unwrap_or_else(|| panic!("schema pointer {ptr} missing"))
            .as_u64()
            .unwrap_or_else(|| panic!("schema pointer {ptr} is not an integer"))
    };
    assert_eq!(
        sch.pointer("/properties/version/const").unwrap(),
        c("ENVELOPE_VERSION")
    );
    assert_eq!(
        at("/properties/activity/properties/raw_body/maxLength"),
        c("RAW_BODY_BASE64_MAX_CHARS")
    );
    assert_eq!(
        c("RAW_BODY_BASE64_MAX_CHARS"),
        4 * c("INBOX_BODY_MAX_BYTES").div_ceil(3)
    );
    assert_eq!(at("/$defs/opaque_id/minLength"), c("OPAQUE_ID_MIN"));
    assert_eq!(at("/$defs/opaque_id/maxLength"), c("OPAQUE_ID_MAX"));
    assert_eq!(at("/$defs/origin/maxLength"), c("ORIGIN_MAX"));
    assert_eq!(at("/$defs/remote_url/maxLength"), c("URL_MAX"));
    assert_eq!(at("/$defs/emumet_url/maxLength"), c("URL_MAX"));
    assert_eq!(
        at("/properties/sender/properties/instance_actor/maxLength"),
        c("URL_MAX")
    );
    assert_eq!(
        at("/properties/original_request/properties/request_target/maxLength"),
        c("REQUEST_TARGET_MAX")
    );
    assert_eq!(
        at("/properties/original_request/properties/signature_headers/maxItems"),
        c("EVIDENCE_MAX_HEADERS")
    );
    assert_eq!(
        at("/$defs/header_name/maxLength"),
        c("EVIDENCE_HEADER_NAME_MAX")
    );
    assert_eq!(
        at("/properties/original_request/properties/signature_headers/items/properties/value/maxLength"),
        c("EVIDENCE_HEADER_VALUE_MAX")
    );
    assert_eq!(
        at("/properties/omitted_header_names/maxItems"),
        c("OMITTED_NAMES_MAX")
    );
    assert_eq!(
        at("/properties/omitted_header_count/maximum"),
        c("OMITTED_COUNT_MAX")
    );
    assert_eq!(
        at("/properties/recipient/properties/link_generation/maximum"),
        c("LINK_GENERATION_MAX")
    );
}

#[test]
fn schema_is_valid_and_boundaries_hold() {
    let v = validator();
    let base = case("p1", "p1-ok-minimal");
    let env: Value = serde_json::from_str(request_body(&base["request"]).as_str()).unwrap();
    assert!(v.is_valid(&env), "the minimal fixture envelope validates");

    let with_len = |n: usize| {
        let mut e = env.clone();
        e["activity"]["raw_body"] = Value::String(B64.encode(vec![0u8; n]));
        e
    };
    let limit = constant("INBOX_BODY_MAX_BYTES") as usize;
    assert!(v.is_valid(&with_len(limit)), "1 MiB fits the schema");
    // Base64 length is rounded up to whole groups, so the schema also lets 1 MiB + 1 pass; the exact
    // size is a check outside the schema.
    assert!(v.is_valid(&with_len(limit + 1)));
    assert!(
        !v.is_valid(&with_len(limit + 3)),
        "one more Base64 group exceeds the schema bound"
    );
    assert_eq!(decoded_len(&with_len(limit)), limit);
    assert!(post_auth_checks(&with_len(limit + 1)).is_err());
    assert_eq!(
        post_auth_checks(&with_len(limit + 1)).unwrap_err().1,
        "activity_too_large"
    );
    assert!(post_auth_checks(&with_len(limit)).is_ok());

    // Evidence budget: exactly the budget passes, one byte more fails.
    let budget = constant("EVIDENCE_BUDGET_BYTES") as usize;
    let with_evidence = |n: usize| {
        // two headers named "x" and "y": the budget counts 2 name bytes plus every value byte
        let value_bytes = n - 2;
        let first = value_bytes.min(16384);
        let mut e = env.clone();
        e["original_request"]["signature_headers"] = serde_json::json!([
            {"name": "x", "value": "a".repeat(first)},
            {"name": "y", "value": "b".repeat(value_bytes - first)},
        ]);
        e
    };
    let at_budget = with_evidence(budget);
    assert_eq!(evidence_bytes(&at_budget), budget);
    assert!(v.is_valid(&at_budget));
    assert!(post_auth_checks(&at_budget).is_ok());
    let over = with_evidence(budget + 1);
    assert_eq!(evidence_bytes(&over), budget + 1);
    assert_eq!(
        post_auth_checks(&over).unwrap_err().1,
        "evidence_budget_exceeded"
    );
}

// ---------------------------------------------------------------------------------------------
// reference receiver (v1.md section 7)
// ---------------------------------------------------------------------------------------------

fn case(dir: &str, id: &str) -> Value {
    read_json(&fixtures_dir().join(dir).join(format!("{id}.json")))
}

fn request_body(request: &Value) -> String {
    if let Some(synth) = request.get("body_synthetic") {
        assert_eq!(synth["kind"], "repeat");
        let unit = s(synth, "unit");
        return unit.repeat(synth["bytes"].as_u64().unwrap() as usize);
    }
    s(request, "body").to_string()
}

fn decoded_len(env: &Value) -> usize {
    B64.decode(env["activity"]["raw_body"].as_str().unwrap())
        .map(|v| v.len())
        .unwrap_or(usize::MAX)
}

fn evidence_bytes(env: &Value) -> usize {
    let headers: usize = env["original_request"]["signature_headers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| s(h, "name").len() + s(h, "value").len())
        .sum();
    let omitted: usize = env["omitted_header_names"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap().len())
        .sum();
    headers + omitted
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        _ => 0,
    }
}

/// v1.md 4.5 / 4.4 item 5: the day exists in that month and year. Meant for strings that already
/// passed the schema pattern.
fn calendar_ok(ts: &str) -> bool {
    let (Some(y), Some(m), Some(d)) = (ts.get(0..4), ts.get(5..7), ts.get(8..10)) else {
        return false;
    };
    let (Ok(y), Ok(m), Ok(d)) = (y.parse::<i64>(), m.parse::<i64>(), d.parse::<i64>()) else {
        return false;
    };
    (1..=days_in_month(y, m)).contains(&d)
}

/// v1.md 4.4 items 2, 3 and 5.
fn post_auth_checks(env: &Value) -> Result<(), (u16, &'static str)> {
    if decoded_len(env) > constant("INBOX_BODY_MAX_BYTES") as usize {
        return Err((413, "activity_too_large"));
    }
    if evidence_bytes(env) > constant("EVIDENCE_BUDGET_BYTES") as usize {
        return Err((400, "evidence_budget_exceeded"));
    }
    for ts in [&env["received_at"], &env["verification"]["verified_at"]] {
        if !calendar_ok(ts.as_str().unwrap_or("")) {
            return Err((400, "schema_violation"));
        }
    }
    Ok(())
}

/// v1.md 4.5: host, optional port and what follows the authority of an envelope URL.
fn url_parts(value: &str, allow_http: bool, lowercase: bool) -> Option<(&str, Option<u32>, &str)> {
    if value.chars().count() > constant("URL_MAX") as usize
        || !value.bytes().all(|b| (0x21..=0x7e).contains(&b))
    {
        return None;
    }
    let rest = value
        .strip_prefix("https://")
        .or_else(|| allow_http.then(|| value.strip_prefix("http://")).flatten())?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let (host, port) = match authority.split_once(':') {
        Some((h, p)) => (h, Some(p)),
        None => (authority, None),
    };
    let alnum = |c: char| c.is_ascii_alphanumeric() && !(lowercase && c.is_ascii_uppercase());
    let sep = |c: char| c == '.' || c == '-';
    let chars: Vec<char> = host.chars().collect();
    let host_ok = !chars.is_empty()
        && alnum(chars[0])
        && alnum(chars[chars.len() - 1])
        && chars.iter().all(|&c| alnum(c) || sep(c))
        && chars.windows(2).all(|w| !(sep(w[0]) && sep(w[1])));
    if !host_ok {
        return None;
    }
    let port = match port {
        None => None,
        Some(p) => {
            if p.is_empty()
                || p.len() > 5
                || p.starts_with('0')
                || !p.bytes().all(|b| b.is_ascii_digit())
            {
                return None;
            }
            let n: u32 = p.parse().ok()?;
            if !(1..=65535).contains(&n) {
                return None;
            }
            Some(n)
        }
    };
    Some((host, port, tail))
}

fn is_remote_url(value: &str) -> bool {
    url_parts(value, true, false).is_some()
}

/// The schema definition named by `def` in the field-format fixtures.
fn url_matches_def(def: &str, value: &str) -> bool {
    match def {
        "remote_url" => is_remote_url(value),
        "emumet_url" => {
            url_parts(value, false, true).is_some_and(|(_, _, tail)| tail.starts_with('/'))
        }
        "origin" => {
            value.chars().count() <= constant("ORIGIN_MAX") as usize
                && url_parts(value, false, true)
                    .is_some_and(|(_, port, tail)| tail.is_empty() && port != Some(443))
        }
        "instance_actor" => {
            url_parts(value, false, true).is_some_and(|(_, _, tail)| tail == "/ap/instance")
        }
        other => panic!("unknown definition {other}"),
    }
}

fn minimal_envelope() -> Value {
    let c = case("p1", "p1-ok-minimal");
    serde_json::from_str(&request_body(&c["request"])).unwrap()
}

fn origin_of(url: &str) -> String {
    let rest = url.strip_prefix("https://").unwrap_or(url);
    let end = rest.find('/').unwrap_or(rest.len());
    format!("https://{}", &rest[..end])
}

fn parse_signature_header(value: &str) -> Option<Vec<(String, String)>> {
    let mut out = Vec::new();
    let mut rest = value.trim();
    while !rest.is_empty() {
        let eq = rest.find('=')?;
        let key = rest[..eq].trim().to_string();
        let after = rest[eq + 1..].strip_prefix('"')?;
        let close = after.find('"')?;
        out.push((key, after[..close].to_string()));
        rest = after[close + 1..].trim_start_matches(',').trim();
    }
    Some(out)
}

fn rfc3339_to_unix(ts: &str) -> i64 {
    // YYYY-MM-DDTHH:MM:SS[.fraction]Z
    let b = ts.as_bytes();
    let num = |a: usize, z: usize| -> i64 { ts[a..z].parse().unwrap() };
    assert!(b[10] == b'T' && ts.ends_with('Z'), "timestamp {ts}");
    let (y, m, d) = (num(0, 4), num(5, 7), num(8, 10));
    let (hh, mm, ss) = (num(11, 13), num(14, 16), num(17, 19));
    // days from civil (Howard Hinnant)
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    days * 86_400 + hh * 3600 + mm * 60 + ss
}

fn http_date_to_unix(date: &str) -> Option<i64> {
    let t = httpdate::parse_http_date(date).ok()?;
    Some(t.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64)
}

fn key_from_pem(pem: &str) -> RsaPublicKey {
    RsaPublicKey::from_public_key_pem(pem).expect("public key PEM")
}

fn rsa_verify(key: &RsaPublicKey, message: &str, signature_b64: &str) -> bool {
    let Ok(sig) = B64.decode(signature_b64) else {
        return false;
    };
    let digest = Sha256::digest(message.as_bytes());
    key.verify(Pkcs1v15Sign::new::<Sha256>(), &digest, &sig)
        .is_ok()
}

fn digest_header_of(body: &[u8]) -> String {
    format!("SHA-256={}", B64.encode(Sha256::digest(body)))
}

const ENDPOINT: &str = "/internal/v1/emumet/activities";

/// v1.md section 5: built from the method and target of the request actually received.
fn signing_string(method: &str, target: &str, host: &str, date: &str, digest: &str) -> String {
    let method = method.to_ascii_lowercase();
    format!("(request-target): {method} {target}\nhost: {host}\ndate: {date}\ndigest: {digest}")
}

#[derive(Debug, PartialEq, Eq)]
struct Outcome {
    status: u16,
    reason: String,
}

fn out(status: u16, reason: &str) -> Outcome {
    Outcome {
        status,
        reason: reason.to_string(),
    }
}

/// A receiver that holds no link ledger: its only inputs are `receiver` of the fixture.
fn reference_receive(
    receiver: &Value,
    request: &Value,
    now: &str,
    validator: &jsonschema::Validator,
) -> Outcome {
    let headers = request["headers"].as_object().unwrap();
    let header = |n: &str| headers.get(n).and_then(Value::as_str);
    let body = request_body(request);
    let body = body.as_bytes();

    // step 0
    let (method, target) = (s(request, "method"), s(request, "target"));
    if target != ENDPOINT {
        return out(404, "not_found");
    }
    if method != "POST" {
        return out(405, "method_not_allowed");
    }
    let ct = header("content-type").unwrap_or("").to_ascii_lowercase();
    if ct != "application/json" && ct != "application/json; charset=utf-8" {
        return out(415, "unsupported_media_type");
    }
    if body.len() > constant("ENVELOPE_MAX_BYTES") as usize {
        return out(413, "envelope_too_large");
    }
    // step 1
    let Some(sig_header) = header("signature") else {
        return out(401, "signature_missing");
    };
    let Some(params) = parse_signature_header(sig_header) else {
        return out(401, "signature_malformed");
    };
    let param = |k: &str| params.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    let (Some(key_id), Some(algorithm), Some(signed), Some(sig_b64)) = (
        param("keyId"),
        param("algorithm"),
        param("headers"),
        param("signature"),
    ) else {
        return out(401, "signature_malformed");
    };
    if algorithm != "rsa-sha256" || signed != "(request-target) host date digest" {
        return out(401, "signature_unsupported");
    }
    let key_origin = origin_of(key_id);
    let trusted: Vec<&str> = receiver["trusted_emumet_origins"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o.as_str().unwrap())
        .collect();
    if !trusted.contains(&key_origin.as_str()) {
        return out(401, "untrusted_key_origin");
    }
    // step 2
    let doc = &receiver["instance_actor_documents"][key_origin.as_str()];
    let instance_actor = format!("{key_origin}/ap/instance");
    if doc["id"] != instance_actor.as_str() {
        return out(401, "unknown_key");
    }
    let Some(key) = doc["publicKey"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["id"] == key_id)
    else {
        return out(401, "unknown_key");
    };
    if key["owner"] != instance_actor.as_str() {
        return out(401, "key_owner_mismatch");
    }
    // step 3
    let own_origin = s(receiver, "own_origin");
    let host = header("host").unwrap_or("");
    if Some(host) != own_origin.strip_prefix("https://") {
        return out(401, "host_mismatch");
    }
    let Some(digest) = header("digest") else {
        return out(401, "digest_missing");
    };
    if digest != digest_header_of(body) {
        return out(401, "digest_mismatch");
    }
    let date = header("date").unwrap_or("");
    let Some(date_unix) = http_date_to_unix(date) else {
        return out(401, "date_invalid");
    };
    if (date_unix - rfc3339_to_unix(now)).unsigned_abs() > constant("DATE_TOLERANCE_SECONDS") {
        return out(401, "date_skew");
    }
    let message = signing_string(method, target, host, date, digest);
    if !rsa_verify(&key_from_pem(s(key, "publicKeyPem")), &message, sig_b64) {
        return out(401, "signature_invalid");
    }
    // step 4
    let Ok(env) = serde_json::from_slice::<Value>(body) else {
        return out(400, "invalid_json");
    };
    if let Some(v) = env.get("version").and_then(Value::as_i64) {
        if v != constant("ENVELOPE_VERSION") as i64 {
            return out(400, "unsupported_version");
        }
    }
    if !validator.is_valid(&env) {
        return out(400, "schema_violation");
    }
    if let Err((status, reason)) = post_auth_checks(&env) {
        return out(status, reason);
    }
    // step 5
    if s(&env["sender"], "origin") != key_origin {
        return out(403, "sender_origin_mismatch");
    }
    if s(&env["sender"], "instance_actor") != instance_actor {
        return out(403, "instance_actor_mismatch");
    }
    if origin_of(s(&env["recipient"], "profile_actor")) != s(&env["sender"], "origin") {
        return out(403, "profile_origin_mismatch");
    }
    if s(&env["recipient"], "origin") != own_origin {
        return out(403, "recipient_origin_mismatch");
    }
    // step 6 (P1): link_id / link_generation are not looked at. There is no ledger parameter.
    // step 7
    let already = receiver["already_accepted_receipts"].as_array().unwrap();
    let duplicate = already.iter().any(|r| {
        r["sender_origin"] == env["sender"]["origin"] && r["receipt_id"] == env["receipt_id"]
    });
    if duplicate {
        out(200, "duplicate")
    } else {
        out(202, "accepted")
    }
}

fn sender_class(status: u16) -> &'static str {
    match status {
        200..=299 => "success",
        401 | 408 | 429 | 500..=599 => "retry",
        _ => "permanent",
    }
}

fn check_expectation(id: &str, expect: &Value, got: &Outcome) {
    assert_eq!(
        got.status as u64,
        expect["status"].as_u64().unwrap(),
        "{id}: status ({got:?})"
    );
    assert_eq!(got.reason, s(expect, "reason"), "{id}: reason");
    assert_eq!(
        sender_class(got.status),
        s(expect, "sender_class"),
        "{id}: sender class"
    );
    let accepted = (200..300).contains(&got.status);
    assert_eq!(accepted, expect["result"] == "accept", "{id}: result");
}

#[test]
fn p1_cases_run_without_a_link_ledger() {
    let v = validator();
    let cases = json_files_in(&fixtures_dir().join("p1"));
    assert!(
        cases.len() >= 25,
        "expected the full P1 fixture set, got {}",
        cases.len()
    );
    let mut accepted = 0;
    let mut rejected = 0;
    for (name, c) in &cases {
        let id = s(c, "id");
        assert_eq!(format!("{id}.json"), *name, "file name matches id");
        assert_eq!(c["phase"], "P1", "{id}");
        assert_eq!(
            c["preconditions"]["link_ledger"], "none",
            "{id}: P1 holds no ledger"
        );
        let got = reference_receive(&c["receiver"], &c["request"], s(&c["receiver"], "now"), &v);
        check_expectation(id, &c["expect"], &got);
        if c["expect"]["result"] == "accept" {
            accepted += 1
        } else {
            rejected += 1
        }
    }
    assert!(
        accepted >= 8 && rejected >= 20,
        "accepted {accepted}, rejected {rejected}"
    );
}

#[test]
fn p1_has_no_generation_or_ledger_rejection() {
    for (_, c) in json_files_in(&fixtures_dir().join("p1")) {
        let reason = s(&c["expect"], "reason");
        let text = format!("{reason} {}", s(&c, "title")).to_ascii_lowercase();
        assert!(
            !reason.contains("generation") && !reason.starts_with("link_"),
            "P1 fixture {} must not depend on link state",
            s(&c, "id")
        );
        assert!(!text.contains("older generation") && !text.contains("stale generation"));
    }
}

#[test]
fn p2_cases_are_separate_and_state_their_ledger() {
    let v = validator();
    let cases = json_files_in(&fixtures_dir().join("p2"));
    assert!(cases.len() >= 4);
    for (_, c) in &cases {
        let id = s(c, "id");
        assert_eq!(c["phase"], "P2", "{id}");
        assert_eq!(c["status"], "informative-reserved-for-P2", "{id}");
        let ledger = &c["preconditions"]["link_ledger"];
        assert!(
            ledger.is_object() && ledger["links"].is_array(),
            "{id}: ledger must be stated"
        );
        // Without a ledger a P1 receiver accepts these envelopes: the P2 reasons are not P1 behaviour.
        let got = reference_receive(&c["receiver"], &c["request"], s(&c["receiver"], "now"), &v);
        assert_eq!(
            got,
            out(202, "accepted"),
            "{id}: valid and signed, so P1 accepts it"
        );
    }
}

#[test]
fn stale_received_at_is_accepted() {
    let c = case("p1", "p1-ok-stale-received-at");
    let env: Value = serde_json::from_str(&request_body(&c["request"])).unwrap();
    let age = rfc3339_to_unix(s(&c["receiver"], "now")) - rfc3339_to_unix(s(&env, "received_at"));
    assert!(
        age >= 33 * 24 * 3600,
        "received_at is about 33 days old, got {age} s"
    );
    assert_eq!(c["expect"]["status"], 202);
    let date = http_date_to_unix(s(&c["request"]["headers"], "date")).unwrap();
    assert!(
        (date - rfc3339_to_unix(s(&c["receiver"], "now"))).abs() < 60,
        "the outer Date is fresh"
    );
}

#[test]
fn date_tolerance_boundary_matches_the_constant() {
    let tol = constant("DATE_TOLERANCE_SECONDS") as i64;
    for (id, offset) in [
        ("p1-ok-date-at-tolerance", tol),
        ("p1-reject-date-skew", tol + 1),
    ] {
        let c = case("p1", id);
        let date = http_date_to_unix(s(&c["request"]["headers"], "date")).unwrap();
        assert_eq!(
            date - rfc3339_to_unix(s(&c["receiver"], "now")),
            offset,
            "{id}"
        );
    }
}

#[test]
fn size_fixture_uses_the_constant() {
    let c = case("p1", "p1-reject-envelope-too-large");
    assert_eq!(
        c["request"]["body_synthetic"]["bytes"].as_u64().unwrap(),
        constant("ENVELOPE_MAX_BYTES") + 1
    );
}

#[test]
fn retransmission_changes_only_date_and_signature() {
    let v = validator();
    let t = read_json(&fixtures_dir().join("retransmission.json"));
    assert_eq!(t["preconditions"]["link_ledger"], "none");
    let attempts = t["receiver_at_attempt"].as_array().unwrap();
    assert_eq!(attempts.len(), 3);
    let first = &attempts[0]["request"];
    for a in attempts {
        let req = &a["request"];
        assert_eq!(
            req["body"], first["body"],
            "the envelope bytes never change"
        );
        assert_eq!(req["headers"]["digest"], first["headers"]["digest"]);
        assert_eq!(req["target"], first["target"]);
        let mut receiver = t["receiver"].clone();
        if let Some(already) = a.get("already_accepted_receipts") {
            receiver["already_accepted_receipts"] = already.clone();
        }
        let got = reference_receive(&receiver, req, s(a, "now"), &v);
        check_expectation("retransmission", &a["expect"], &got);
    }
    for later in &attempts[1..] {
        assert_ne!(
            later["request"]["headers"]["date"],
            first["headers"]["date"]
        );
        assert_ne!(
            later["request"]["headers"]["signature"],
            first["headers"]["signature"]
        );
    }
    let env: Value = serde_json::from_str(s(first, "body")).unwrap();
    assert!(!s(&env, "receipt_id").is_empty());
    assert!(
        t["unchanged"].as_array().unwrap().len() >= 4
            && !t["changed"].as_array().unwrap().is_empty()
    );
}

#[test]
fn signature_vectors_verify() {
    let doc = read_json(&fixtures_dir().join("signature-vectors.json"));
    let vectors = doc["vectors"].as_array().unwrap();
    assert!(vectors.len() >= 7);
    let mut mismatch = 0;
    for vec in vectors {
        let id = s(vec, "id");
        let key = key_from_pem(&read_text(&fixtures_dir().join(s(vec, "public_key_file"))));
        let body = s(vec, "body");
        assert_eq!(
            B64.encode(Sha256::digest(body.as_bytes())),
            s(vec, "body_sha256_base64"),
            "{id}"
        );
        // What a receiver does: build the signing string from the request as received.
        let rebuilt = signing_string(
            s(vec, "method"),
            s(vec, "target"),
            s(vec, "host"),
            s(vec, "date"),
            s(vec, "digest_header"),
        );
        assert_eq!(
            s(vec, "signed_headers"),
            "(request-target) host date digest"
        );
        let params = parse_signature_header(s(vec, "signature_header")).unwrap();
        let sig = &params.iter().find(|(k, _)| k == "signature").unwrap().1;
        assert_eq!(
            params.iter().find(|(k, _)| k == "keyId").unwrap().1,
            s(vec, "key_id")
        );
        let digest_ok = s(vec, "digest_header") == digest_header_of(body.as_bytes());
        let verifies_rebuilt = rsa_verify(&key, &rebuilt, sig);
        let verifies_stored = rsa_verify(&key, s(vec, "signing_string"), sig);
        match s(vec, "expect") {
            "valid" => {
                assert_eq!(rebuilt, s(vec, "signing_string"), "{id}: signing string");
                assert!(digest_ok && verifies_rebuilt, "{id}");
            }
            "digest_mismatch" => {
                assert_eq!(rebuilt, s(vec, "signing_string"), "{id}: signing string");
                assert!(!digest_ok && verifies_rebuilt, "{id}");
            }
            "signature_invalid" => {
                assert_eq!(rebuilt, s(vec, "signing_string"), "{id}: signing string");
                assert!(digest_ok && !verifies_rebuilt, "{id}");
            }
            "signing_string_mismatch" => {
                // The signature is good for another method / target, not for this request.
                mismatch += 1;
                assert_ne!(rebuilt, s(vec, "signing_string"), "{id}");
                assert!(digest_ok && verifies_stored && !verifies_rebuilt, "{id}");
            }
            other => panic!("{id}: unknown expectation {other}"),
        }
    }
    assert_eq!(mismatch, 2, "other-target and other-method vectors");
}

#[test]
fn original_signature_evidence_can_be_verified_again() {
    let c = case("p1", "p1-ok-minimal");
    let env: Value = serde_json::from_str(&request_body(&c["request"])).unwrap();
    assert_eq!(env["evidence_complete"], true);
    let raw = B64.decode(s(&env["activity"], "raw_body")).unwrap();
    let headers = env["original_request"]["signature_headers"]
        .as_array()
        .unwrap();
    let get = |n: &str| {
        headers
            .iter()
            .find(|h| h["name"] == n)
            .map(|h| s(h, "value").to_string())
            .unwrap()
    };
    assert_eq!(
        get("digest"),
        digest_header_of(&raw),
        "the stored digest describes the stored raw body"
    );
    let params = parse_signature_header(&get("signature")).unwrap();
    let pval = |k: &str| params.iter().find(|(n, _)| n == k).unwrap().1.clone();
    assert_eq!(pval("keyId"), s(&env["verification"], "key_id"));
    let msg = signing_string(
        s(&env["original_request"], "method"),
        s(&env["original_request"], "request_target"),
        &get("host"),
        &get("date"),
        &get("digest"),
    );
    let key = key_from_pem(&read_text(&fixtures_dir().join("keys/remote-bob.pub.pem")));
    assert!(
        rsa_verify(&key, &msg, &pval("signature")),
        "the original signature verifies from the evidence alone"
    );
    // activity.id / actor are what the raw body says
    let body: Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(body["id"], env["activity"]["id"]);
    assert_eq!(body["actor"], env["activity"]["actor"]);
}

#[test]
fn key_index_is_consistent() {
    let keys = read_json(&fixtures_dir().join("keys/keys.json"));
    for (name, k) in keys["keys"].as_object().unwrap() {
        let pem = fixtures_dir().join(s(k, "public_key_file"));
        assert!(pem.exists(), "{name}");
        let key = key_from_pem(&read_text(&pem));
        assert_eq!(
            rsa::traits::PublicKeyParts::size(&key),
            256,
            "{name} is RSA-2048"
        );
    }
    let text = read_text(&fixtures_dir().join("keys/keys.json"));
    assert!(!text.contains("PRIVATE KEY"));
    for e in fs::read_dir(fixtures_dir().join("keys")).unwrap() {
        let p = e.unwrap().path();
        assert!(
            !read_text(&p).contains("PRIVATE KEY"),
            "{} must not hold a private key",
            p.display()
        );
    }
}

// ---------------------------------------------------------------------------------------------
// reference reader (v1.md section 8)
// ---------------------------------------------------------------------------------------------

/// JSON value that rejects duplicate object keys at every depth (keys compared after unescaping).
struct Strict(Value);

impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Strict;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("any JSON value")
            }
            fn visit_bool<E>(self, v: bool) -> Result<Strict, E> {
                Ok(Strict(Value::Bool(v)))
            }
            fn visit_i64<E>(self, v: i64) -> Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_u64<E>(self, v: u64) -> Result<Strict, E> {
                Ok(Strict(v.into()))
            }
            fn visit_f64<E>(self, v: f64) -> Result<Strict, E> {
                Ok(Strict(
                    serde_json::Number::from_f64(v).map_or(Value::Null, Value::Number),
                ))
            }
            fn visit_str<E>(self, v: &str) -> Result<Strict, E> {
                Ok(Strict(Value::String(v.to_string())))
            }
            fn visit_string<E>(self, v: String) -> Result<Strict, E> {
                Ok(Strict(Value::String(v)))
            }
            fn visit_unit<E>(self) -> Result<Strict, E> {
                Ok(Strict(Value::Null))
            }
            fn visit_none<E>(self) -> Result<Strict, E> {
                Ok(Strict(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Strict, A::Error> {
                let mut items = Vec::new();
                while let Some(Strict(v)) = seq.next_element()? {
                    items.push(v);
                }
                Ok(Strict(Value::Array(items)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Strict, A::Error> {
                let mut m = Map::new();
                while let Some((k, Strict(v))) = map.next_entry::<String, Strict>()? {
                    if m.insert(k, v).is_some() {
                        return Err(de::Error::custom("duplicate_key"));
                    }
                }
                Ok(Strict(Value::Object(m)))
            }
        }
        d.deserialize_any(V)
    }
}

fn depth_of(v: &Value) -> usize {
    match v {
        Value::Array(a) => 1 + a.iter().map(depth_of).max().unwrap_or(0),
        Value::Object(o) => 1 + o.values().map(depth_of).max().unwrap_or(0),
        _ => 0,
    }
}

fn nodes_of(v: &Value) -> usize {
    match v {
        Value::Array(a) => 1 + a.iter().map(nodes_of).sum::<usize>(),
        Value::Object(o) => 1 + o.values().map(nodes_of).sum::<usize>(),
        _ => 1,
    }
}

const AS_IRI: &str = "https://www.w3.org/ns/activitystreams#";

/// The recognized ActivityStreams term of a `type` string, if any.
fn as_term(t: &str) -> Option<&str> {
    if let Some(rest) = t.strip_prefix(AS_IRI) {
        return Some(rest);
    }
    if let Some(rest) = t.strip_prefix("as:") {
        return Some(rest);
    }
    (!t.contains(':') && !t.contains('/')).then_some(t)
}

fn type_string_ok(t: &str) -> bool {
    !t.is_empty() && t.chars().count() <= constant("TYPE_STRING_MAX") as usize
}

fn type_strings(v: &Value) -> Option<Vec<String>> {
    match v {
        Value::String(t) if type_string_ok(t) => Some(vec![t.clone()]),
        Value::Array(a) if !a.is_empty() && a.len() <= constant("TYPE_ARRAY_MAX") as usize => a
            .iter()
            .map(|e| e.as_str().filter(|t| type_string_ok(t)).map(str::to_string))
            .collect(),
        _ => None,
    }
}

fn actor_id(v: &Value) -> Option<String> {
    match v {
        Value::String(u) => Some(u.clone()),
        Value::Object(o) => o.get("id").and_then(Value::as_str).map(str::to_string),
        _ => None,
    }
}

fn media_type_ok(ct: &str) -> bool {
    let mut parts = ct.split(';');
    let media = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    if media != "application/activity+json" && media != "application/ld+json" {
        return false;
    }
    for p in parts {
        let Some((k, v)) = p.split_once('=') else {
            return false;
        };
        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().trim_matches('"'));
        let ok = (k == "charset" && v.eq_ignore_ascii_case("utf-8"))
            || (media == "application/ld+json"
                && k == "profile"
                && v == "https://www.w3.org/ns/activitystreams");
        if !ok {
            return false;
        }
    }
    true
}

#[derive(Debug, PartialEq)]
enum Reading {
    Forward {
        activity_id: Option<String>,
        actor: String,
    },
    Exclude(&'static str),
    Reject(u16, &'static str),
}

fn synthetic_body(spec: &Value) -> String {
    const HEAD: &str = r#"{"type":"Create","actor":"https://remote.example/users/bob","#;
    match s(spec, "kind") {
        "padding" => {
            let prefix = format!("{HEAD}\"pad\":\"");
            let total = spec["bytes"].as_u64().unwrap() as usize;
            format!("{prefix}{}\"}}", "a".repeat(total - prefix.len() - 2))
        }
        "nested_depth" => {
            let n = spec["depth"].as_u64().unwrap() as usize - 1;
            format!("{HEAD}\"x\":{}{}}}", "[".repeat(n), "]".repeat(n))
        }
        "node_count" => {
            let zeros = spec["nodes"].as_u64().unwrap() as usize - 4;
            format!("{HEAD}\"x\":[{}]}}", vec!["0"; zeros].join(","))
        }
        other => panic!("unknown synthetic kind {other}"),
    }
}

fn read_inbox(c: &Value) -> Reading {
    let body = c
        .get("body_synthetic")
        .map_or_else(|| s(c, "body").to_string(), synthetic_body);
    if body.len() > constant("INBOX_BODY_MAX_BYTES") as usize {
        return Reading::Reject(413, "body_too_large");
    }
    if !media_type_ok(s(c, "content_type")) {
        return Reading::Reject(400, "invalid_content_type");
    }
    let parsed = match serde_json::from_str::<Strict>(&body) {
        Ok(Strict(v)) => v,
        Err(e) if e.to_string().starts_with("duplicate_key") => {
            return Reading::Reject(400, "duplicate_key")
        }
        Err(_) => return Reading::Reject(400, "invalid_json"),
    };
    let Some(obj) = parsed.as_object() else {
        return Reading::Reject(400, "not_object");
    };
    if depth_of(&parsed) > constant("JSON_MAX_DEPTH") as usize {
        return Reading::Reject(400, "too_deep");
    }
    if nodes_of(&parsed) > constant("JSON_MAX_NODES") as usize {
        return Reading::Reject(400, "too_many_nodes");
    }
    let Some(types) = obj.get("type").and_then(type_strings) else {
        return Reading::Reject(400, "invalid_type");
    };
    let actor = match obj.get("actor") {
        Some(Value::Array(a))
            if !a.is_empty() && a.len() <= constant("ACTOR_ARRAY_MAX") as usize =>
        {
            let ids: Option<HashSet<String>> = a.iter().map(actor_id).collect();
            ids.filter(|s| s.len() == 1)
                .and_then(|s| s.into_iter().next())
        }
        Some(Value::Array(_)) | None => None,
        Some(other) => actor_id(other),
    };
    let Some(actor) = actor else {
        return Reading::Reject(400, "actor_missing_or_ambiguous");
    };
    if actor.chars().count() > constant("URL_MAX") as usize || !is_remote_url(&actor) {
        return Reading::Reject(400, "invalid_actor");
    }
    let relations: BTreeSet<&str> = types
        .iter()
        .filter_map(|t| as_term(t))
        .filter(|t| matches!(*t, "Follow" | "Block" | "Undo" | "Accept" | "Reject"))
        .collect();
    if relations.len() >= 2 {
        return Reading::Reject(400, "ambiguous_relation");
    }
    let known: HashSet<&str> = c["known_relation_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    // c["prior_undo_markers"] is deliberately never consulted: a marker is not evidence of a Follow.
    let certain = |r: &Value| match r {
        Value::String(u) => known.contains(u.as_str()),
        Value::Object(o) => {
            let by_type = o.get("type").and_then(type_strings).is_some_and(|ts| {
                ts.iter()
                    .filter_map(|t| as_term(t))
                    .any(|t| t == "Follow" || t == "Block")
            });
            by_type
                || o.get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|i| known.contains(i))
        }
        _ => false,
    };
    match relations.iter().next().copied() {
        Some("Follow" | "Block") => Reading::Exclude("type"),
        Some(_) => {
            let refs: Vec<&Value> = match obj.get("object") {
                None => vec![],
                Some(Value::Array(a)) => a.iter().collect(),
                Some(one) => vec![one],
            };
            if !refs.is_empty() && refs.iter().all(|r| certain(r)) {
                Reading::Exclude("known_reference")
            } else {
                forward(obj, actor)
            }
        }
        None => forward(obj, actor),
    }
}

fn forward(obj: &Map<String, Value>, actor: String) -> Reading {
    Reading::Forward {
        activity_id: obj
            .get("id")
            .and_then(Value::as_str)
            .filter(|i| is_remote_url(i))
            .map(str::to_string),
        actor,
    }
}

#[test]
fn inbox_reading_fixtures_match_the_reference_reader() {
    let doc = read_json(&fixtures_dir().join("inbox/activity-reading.json"));
    let cases = doc["cases"].as_array().unwrap();
    assert!(cases.len() >= 60);
    let mut ids = HashSet::new();
    let mut counts = (0, 0, 0);
    for c in cases {
        let id = s(c, "id");
        assert!(ids.insert(id.to_string()), "duplicate case id {id}");
        let expect = &c["expect"];
        let want = match s(expect, "outcome") {
            "forward" => {
                counts.0 += 1;
                Reading::Forward {
                    activity_id: expect["activity_id"].as_str().map(str::to_string),
                    actor: s(expect, "actor").to_string(),
                }
            }
            "exclude" => {
                counts.1 += 1;
                Reading::Exclude(match s(expect, "via") {
                    "type" => "type",
                    "known_reference" => "known_reference",
                    other => panic!("{id}: unknown via {other}"),
                })
            }
            "reject" => {
                counts.2 += 1;
                let status = expect["status"].as_u64().unwrap() as u16;
                assert!(status == 400 || status == 413, "{id}");
                let reason = s(expect, "reason").to_string();
                Reading::Reject(status, Box::leak(reason.into_boxed_str()))
            }
            other => panic!("{id}: unknown outcome {other}"),
        };
        assert_eq!(read_inbox(c), want, "{id}");
    }
    assert!(
        counts.0 >= 25 && counts.1 >= 10 && counts.2 >= 20,
        "{counts:?}"
    );
}

#[test]
fn inbox_boundary_fixtures_use_the_constants() {
    let doc = read_json(&fixtures_dir().join("inbox/activity-reading.json"));
    let find = |id: &str| {
        doc["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == id)
            .unwrap()
            .clone()
    };
    let spec = |id: &str, key: &str| find(id)["body_synthetic"][key].as_u64().unwrap();
    assert_eq!(
        spec("forward-depth-32", "depth"),
        constant("JSON_MAX_DEPTH")
    );
    assert_eq!(
        spec("reject-depth-33", "depth"),
        constant("JSON_MAX_DEPTH") + 1
    );
    assert_eq!(
        spec("forward-nodes-100000", "nodes"),
        constant("JSON_MAX_NODES")
    );
    assert_eq!(
        spec("reject-nodes-100001", "nodes"),
        constant("JSON_MAX_NODES") + 1
    );
    assert_eq!(
        spec("forward-body-exactly-1mib", "bytes"),
        constant("INBOX_BODY_MAX_BYTES")
    );
    assert_eq!(
        spec("reject-body-over-1mib", "bytes"),
        constant("INBOX_BODY_MAX_BYTES") + 1
    );
    let count = |id: &str| {
        type_strings(&serde_json::from_str::<Value>(s(&find(id), "body")).unwrap()["type"])
            .map(|t| t.len())
    };
    assert_eq!(
        count("forward-type-array-16"),
        Some(constant("TYPE_ARRAY_MAX") as usize)
    );
    let body_of = |id: &str| serde_json::from_str::<Value>(s(&find(id), "body")).unwrap();
    let chars = |v: &Value| v.as_str().unwrap().chars().count() as u64;
    assert_eq!(
        chars(&body_of("forward-actor-exactly-url-max")["actor"]),
        constant("URL_MAX")
    );
    assert_eq!(
        chars(&body_of("reject-actor-too-long")["actor"]),
        constant("URL_MAX") + 1
    );
    assert_eq!(
        chars(&body_of("forward-id-exactly-url-max")["id"]),
        constant("URL_MAX")
    );
    assert_eq!(
        chars(&body_of("forward-id-too-long-becomes-null")["id"]),
        constant("URL_MAX") + 1
    );
    assert_eq!(
        chars(&body_of("forward-type-string-exactly-max")["type"]),
        constant("TYPE_STRING_MAX")
    );
    assert_eq!(
        chars(&body_of("reject-type-string-too-long")["type"]),
        constant("TYPE_STRING_MAX") + 1
    );
    let too_many = serde_json::from_str::<Value>(s(&find("reject-type-array-17"), "body")).unwrap();
    assert_eq!(
        too_many["type"].as_array().unwrap().len() as u64,
        constant("TYPE_ARRAY_MAX") + 1
    );
}

#[test]
fn forwarded_inbox_values_are_accepted_by_the_envelope_schema() {
    let v = validator();
    let doc = read_json(&fixtures_dir().join("inbox/activity-reading.json"));
    let env = minimal_envelope();
    let mut forwarded = 0;
    for c in doc["cases"].as_array().unwrap() {
        if let Reading::Forward { activity_id, actor } = read_inbox(c) {
            let mut e = env.clone();
            e["activity"]["id"] = activity_id.map_or(Value::Null, Value::from);
            e["activity"]["actor"] = actor.into();
            assert!(
                v.is_valid(&e),
                "{}: the extracted values must fit the envelope",
                s(c, "id")
            );
            forwarded += 1;
        }
    }
    assert!(forwarded >= 35, "{forwarded}");
    // The limit of the reader is the limit of the schema, in both directions.
    let url_max = constant("URL_MAX") as usize;
    for (len, ok) in [(url_max, true), (url_max + 1, false)] {
        let url = format!(
            "https://remote.example/{}",
            "a".repeat(len - "https://remote.example/".len())
        );
        assert_eq!(url.len(), len);
        let mut e = env.clone();
        e["activity"]["id"] = url.clone().into();
        e["activity"]["actor"] = url.clone().into();
        assert_eq!(v.is_valid(&e), ok, "URL of {len} characters");
        assert_eq!(is_remote_url(&url), ok, "reader URL check at {len}");
    }
}

#[test]
fn field_formats_match_the_schema_and_the_reference_checks() {
    let v = validator();
    let doc = read_json(&fixtures_dir().join("formats/field-formats.json"));
    let env = minimal_envelope();
    let timestamps = doc["timestamps"].as_array().unwrap();
    assert!(timestamps.len() >= 25);
    for t in timestamps {
        let value = s(t, "value");
        for ptr in ["/received_at", "/verification/verified_at"] {
            let mut e = env.clone();
            *e.pointer_mut(ptr).unwrap() = value.into();
            assert_eq!(
                v.is_valid(&e),
                t["schema_valid"] == true,
                "{ptr} {value:?}: schema"
            );
        }
        if t["schema_valid"] == true {
            assert_eq!(
                calendar_ok(value),
                t["calendar_valid"] == true,
                "{value:?}: calendar"
            );
        }
        assert_eq!(
            t["valid"] == true,
            t["schema_valid"] == true && t["calendar_valid"] == true,
            "{value:?}"
        );
    }
    let urls = doc["urls"].as_array().unwrap();
    assert!(urls.len() >= 40);
    for u in urls {
        let (def, value) = (s(u, "def"), s(u, "value"));
        let ptr = format!("/{}", s(&doc["field_for_def"], def).replace('.', "/"));
        let mut e = env.clone();
        *e.pointer_mut(&ptr).unwrap() = value.into();
        let preview: String = value.chars().take(60).collect();
        assert_eq!(
            v.is_valid(&e),
            u["valid"] == true,
            "{def} {preview:?}: schema"
        );
        assert_eq!(
            url_matches_def(def, value),
            u["valid"] == true,
            "{def} {preview:?}: reference check"
        );
    }
}

fn is_secret_header(name: &str) -> bool {
    matches!(
        name,
        "authorization" | "proxy-authorization" | "cookie" | "set-cookie"
    ) || ["token", "secret", "api-key", "apikey", "password"]
        .iter()
        .any(|w| name.contains(w))
}

/// Header names covered by the signature fields (v1.md 4.3 group 2), pseudo-components skipped.
fn covered_names(fields: &[(String, String)]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for (n, v) in fields.iter().filter(|(n, _)| n == "signature") {
        let _ = n;
        if let Some(p) = v.find("headers=\"") {
            let rest = &v[p + "headers=\"".len()..];
            if let Some(end) = rest.find('"') {
                names.extend(rest[..end].split_whitespace().map(str::to_ascii_lowercase));
            }
        }
    }
    for (_, v) in fields.iter().filter(|(n, _)| n == "signature-input") {
        let c: Vec<char> = v.chars().collect();
        let mut i = 0;
        while i < c.len() {
            if c[i] != '(' {
                i += 1;
                continue;
            }
            i += 1;
            loop {
                while i < c.len() && c[i] == ' ' {
                    i += 1;
                }
                if i >= c.len() || c[i] == ')' {
                    i += 1;
                    break;
                }
                if c[i] != '"' {
                    i += 1;
                    continue;
                }
                let Some(len) = c[i + 1..].iter().position(|&x| x == '"') else {
                    i = c.len();
                    break;
                };
                names.push(
                    c[i + 1..i + 1 + len]
                        .iter()
                        .collect::<String>()
                        .to_ascii_lowercase(),
                );
                i += len + 2;
                let mut in_quotes = false;
                while i < c.len() && (in_quotes || (c[i] != ' ' && c[i] != ')')) {
                    if c[i] == '"' {
                        in_quotes = !in_quotes;
                    }
                    i += 1;
                }
            }
        }
    }
    names
        .into_iter()
        .filter(|n| !n.starts_with('(') && !n.starts_with('@'))
        .collect()
}

/// v1.md 4.3, written from the document.
fn build_evidence(original: &[Value]) -> Value {
    let budget = constant("EVIDENCE_BUDGET_BYTES") as usize;
    let mut fields: Vec<(String, String)> = Vec::new();
    for h in original {
        for _ in 0..h.get("times").and_then(Value::as_u64).unwrap_or(1) {
            fields.push((s(h, "name").to_ascii_lowercase(), s(h, "value").to_string()));
        }
    }
    let mut order: Vec<usize> = Vec::new();
    let mut seen: HashSet<usize> = HashSet::new();
    let mut add = |i: usize| {
        if seen.insert(i) {
            order.push(i);
        }
    };
    for (i, (n, _)) in fields.iter().enumerate() {
        if n == "signature" || n == "signature-input" {
            add(i);
        }
    }
    for name in covered_names(&fields) {
        for (i, (n, _)) in fields.iter().enumerate() {
            if *n == name {
                add(i);
            }
        }
    }
    for name in [
        "digest",
        "content-digest",
        "date",
        "host",
        "content-type",
        "content-length",
    ] {
        for (i, (n, _)) in fields.iter().enumerate() {
            if n == name {
                add(i);
            }
        }
    }
    let mut kept: Vec<Value> = Vec::new();
    let mut omitted_names: Vec<String> = Vec::new();
    let (mut used, mut count) = (0usize, 0u64);
    for i in order {
        let (name, value) = &fields[i];
        let unusable = is_secret_header(name)
            || name.chars().count() > constant("EVIDENCE_HEADER_NAME_MAX") as usize
            || !value.bytes().all(|b| (0x20..=0x7e).contains(&b))
            || value.len() > constant("EVIDENCE_HEADER_VALUE_MAX") as usize
            || kept.len() >= constant("EVIDENCE_MAX_HEADERS") as usize
            || used + name.len() + value.len() > budget;
        if unusable {
            count = (count + 1).min(constant("OMITTED_COUNT_MAX"));
            if !omitted_names.contains(name)
                && omitted_names.len() < constant("OMITTED_NAMES_MAX") as usize
                && name.chars().count() <= constant("EVIDENCE_HEADER_NAME_MAX") as usize
                && used + name.len() <= budget
            {
                omitted_names.push(name.clone());
                used += name.len();
            }
        } else {
            kept.push(serde_json::json!({"name": name, "value": value}));
            used += name.len() + value.len();
        }
    }
    serde_json::json!({
        "signature_headers": kept,
        "omitted_header_names": omitted_names,
        "omitted_header_count": count,
        "evidence_complete": count == 0,
    })
}

#[test]
fn header_evidence_algorithm_matches_the_fixtures() {
    let v = validator();
    let doc = read_json(&fixtures_dir().join("evidence/header-omission.json"));
    let cases = doc["cases"].as_array().unwrap();
    assert!(cases.len() >= 14);
    let env = minimal_envelope();
    let mut ids = HashSet::new();
    for c in cases {
        let id = s(c, "id");
        assert!(ids.insert(id), "duplicate case {id}");
        let got = build_evidence(c["original_headers"].as_array().unwrap());
        assert_eq!(got, c["expect"], "{id}");
        // What the algorithm produces is always a valid envelope within the budget.
        let mut e = env.clone();
        e["original_request"]["signature_headers"] = got["signature_headers"].clone();
        e["omitted_header_names"] = got["omitted_header_names"].clone();
        e["omitted_header_count"] = got["omitted_header_count"].clone();
        e["evidence_complete"] = got["evidence_complete"].clone();
        assert!(v.is_valid(&e), "{id}: the evidence fits the schema");
        assert!(
            post_auth_checks(&e).is_ok(),
            "{id}: the evidence fits the budget"
        );
    }
    for needed in [
        "budget-exactly-full",
        "budget-one-over-omits-the-big-header",
        "budget-full-name-not-listed",
        "omitted-names-limit",
        "too-many-headers",
        "count-saturates",
        "candidate-only-once",
        "rfc9421-components",
    ] {
        assert!(ids.contains(needed), "missing case {needed}");
    }
}
