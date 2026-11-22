//! P1 mock ShuttlePub receiver: verifies exactly the contract section 3.3 P1 procedure and keeps NO
//! link state (link id / generation are trusted claims of the configured Emumet).
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub const TARGET: &str = "/internal/v1/emumet/activities";

#[derive(Clone, Debug)]
pub enum Behavior {
    /// Verify like a P1 receiver and answer 202.
    Accept,
    /// Answer this status without verification.
    Status(u16),
    Redirect(String),
    /// Read the request, then never answer (the sender's own deadline must end it).
    Stall,
    /// Verify and durably record the envelope, then drop the connection without a response.
    AcceptThenDisconnect,
}

#[derive(Clone, Debug)]
pub struct Attempt {
    pub body: Vec<u8>,
    pub headers: HashMap<String, String>,
    pub outcome: String,
}

#[derive(Clone, Debug)]
pub struct Accepted {
    pub receipt_id: String,
    pub envelope: Value,
    pub outer_body: Vec<u8>,
    pub raw_body: Vec<u8>,
    pub headers: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct Reject {
    pub status: u16,
    pub code: &'static str,
}

struct State {
    trusted_origin: String,
    own_origin: String,
    own_authority: String,
    fetch_base: Mutex<String>,
    keys: Mutex<HashMap<String, String>>,
    script: Mutex<VecDeque<Behavior>>,
    default: Mutex<Behavior>,
    attempts: Mutex<Vec<Attempt>>,
    accepted: Mutex<Vec<Accepted>>,
    rejected: Mutex<Vec<Reject>>,
    seen: Mutex<HashSet<(String, String)>>,
    validator: jsonschema::Validator,
    fetches: std::sync::atomic::AtomicUsize,
}

#[derive(Clone)]
pub struct Receiver {
    state: Arc<State>,
    pub origin: String,
    pub authority: String,
}

fn schema() -> jsonschema::Validator {
    let path = format!(
        "{}/../docs/contracts/post-relay-v1.schema.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let value: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    jsonschema::validator_for(&value).expect("contract schema compiles")
}

impl Receiver {
    /// `trusted_origin`: the one configured Emumet. `fetch_base`: where that origin's instance
    /// actor is really fetched from over HTTP (the test cannot resolve the HTTPS name).
    pub async fn start(trusted_origin: &str, fetch_base: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let authority = listener.local_addr().unwrap().to_string();
        let origin = format!("http://{authority}");
        let state = Arc::new(State {
            trusted_origin: trusted_origin.into(),
            own_origin: origin.clone(),
            own_authority: authority.clone(),
            fetch_base: Mutex::new(fetch_base.into()),
            keys: Mutex::new(HashMap::new()),
            script: Mutex::new(VecDeque::new()),
            default: Mutex::new(Behavior::Accept),
            attempts: Mutex::new(vec![]),
            accepted: Mutex::new(vec![]),
            rejected: Mutex::new(vec![]),
            seen: Mutex::new(HashSet::new()),
            validator: schema(),
            fetches: Default::default(),
        });
        let s = state.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let s = s.clone();
                tokio::spawn(async move { serve(s, stream).await });
            }
        });
        Self {
            state,
            origin,
            authority,
        }
    }
    pub fn set_fetch_base(&self, base: &str) {
        *self.state.fetch_base.lock().unwrap() = base.into();
        self.state.keys.lock().unwrap().clear();
    }
    pub fn push(&self, behavior: Behavior) {
        self.state.script.lock().unwrap().push_back(behavior);
    }
    pub fn push_n(&self, behavior: Behavior, n: usize) {
        for _ in 0..n {
            self.push(behavior.clone());
        }
    }
    pub fn set_default(&self, behavior: Behavior) {
        *self.state.default.lock().unwrap() = behavior;
    }
    pub fn attempts(&self) -> Vec<Attempt> {
        self.state.attempts.lock().unwrap().clone()
    }
    pub fn accepted(&self) -> Vec<Accepted> {
        self.state.accepted.lock().unwrap().clone()
    }
    pub fn rejected(&self) -> Vec<Reject> {
        self.state.rejected.lock().unwrap().clone()
    }
    pub fn instance_fetches(&self) -> usize {
        self.state.fetches.load(std::sync::atomic::Ordering::SeqCst)
    }
    pub async fn wait_accepted(&self, n: usize, bound: Duration) -> Vec<Accepted> {
        let end = std::time::Instant::now() + bound;
        loop {
            let accepted = self.accepted();
            if accepted.len() >= n {
                return accepted;
            }
            assert!(
                std::time::Instant::now() < end,
                "receiver accepted {} of {n} envelopes in {bound:?}",
                accepted.len()
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    pub async fn wait_attempts(&self, n: usize, bound: Duration) -> Vec<Attempt> {
        let end = std::time::Instant::now() + bound;
        loop {
            let attempts = self.attempts();
            if attempts.len() >= n {
                return attempts;
            }
            assert!(
                std::time::Instant::now() < end,
                "receiver saw {} of {n} attempts in {bound:?}",
                attempts.len()
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    /// The verification entry point, also used directly by the negative-case tests.
    pub async fn verify_request(
        &self,
        method: &str,
        target: &str,
        headers: &HashMap<String, String>,
        body: &[u8],
    ) -> Result<Value, Reject> {
        verify(&self.state, method, target, headers, body)
            .await
            .map(|(_, e)| e)
    }
}

fn reject(status: u16, code: &'static str) -> Reject {
    Reject { status, code }
}

fn origin_of(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    Some(u.origin().ascii_serialization())
}

fn params(header: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut rest = header.trim();
    while let Some((key, after)) = rest.split_once("=\"") {
        let Some((value, tail)) = after.split_once('"') else {
            break;
        };
        out.insert(
            key.trim().trim_start_matches(',').trim().to_string(),
            value.to_string(),
        );
        rest = tail.trim_start_matches(',').trim();
    }
    out
}

async fn instance_key(state: &State, key_id: &str) -> Result<String, Reject> {
    if let Some(pem) = state.keys.lock().unwrap().get(key_id) {
        return Ok(pem.clone());
    }
    let actor_url = key_id.split('#').next().unwrap_or_default();
    // 2. The instance actor and its key are fetched over HTTP (SSRF-safe in production; here a
    //    fixed mapping of the configured origin to the real listener, no redirects).
    let base = state.fetch_base.lock().unwrap().clone();
    let path = actor_url
        .strip_prefix(&state.trusted_origin)
        .ok_or(reject(403, "unknown_emumet"))?;
    state
        .fetches
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let response = client
        .get(format!("{base}{path}"))
        .header("accept", "application/activity+json")
        .send()
        .await
        .map_err(|_| reject(503, "actor_fetch_failed"))?;
    if !response.status().is_success() {
        return Err(reject(401, "actor_fetch_failed"));
    }
    let doc: Value = response
        .json()
        .await
        .map_err(|_| reject(401, "actor_invalid"))?;
    let expected = format!("{}/ap/instance", state.trusted_origin);
    if doc["id"] != expected || actor_url != expected || doc["type"] != "Application" {
        return Err(reject(403, "not_instance_actor"));
    }
    let keys = doc["publicKey"]
        .as_array()
        .ok_or(reject(401, "actor_invalid"))?;
    let key = keys
        .iter()
        .find(|k| k["id"] == key_id)
        .ok_or(reject(401, "unknown_key"))?;
    if key["owner"] != expected {
        return Err(reject(403, "key_owner_mismatch"));
    }
    let pem = key["publicKeyPem"]
        .as_str()
        .ok_or(reject(401, "actor_invalid"))?
        .to_string();
    state
        .keys
        .lock()
        .unwrap()
        .insert(key_id.into(), pem.clone());
    Ok(pem)
}

async fn verify(
    state: &State,
    method: &str,
    target: &str,
    h: &HashMap<String, String>,
    body: &[u8],
) -> Result<(String, Value), Reject> {
    use rsa::pkcs1v15::{Signature, VerifyingKey};
    use rsa::pkcs8::DecodePublicKey;
    use rsa::signature::Verifier;
    if method != "POST" {
        return Err(reject(405, "method_not_allowed"));
    }
    if target != TARGET {
        return Err(reject(404, "not_found"));
    }
    let signature = h.get("signature").ok_or(reject(401, "no_signature"))?;
    let p = params(signature);
    let key_id = p.get("keyId").ok_or(reject(401, "no_key_id"))?;
    // 1. Trusted Emumet: keyId origin must equal the configured origin before anything is fetched.
    if origin_of(key_id).as_deref() != Some(state.trusted_origin.as_str()) {
        return Err(reject(403, "unknown_emumet"));
    }
    let covered: Vec<&str> = p
        .get("headers")
        .map(|s| s.split(' ').collect())
        .unwrap_or_default();
    for need in ["(request-target)", "host", "date", "digest"] {
        if !covered.contains(&need) {
            return Err(reject(401, "signature_does_not_cover_required_headers"));
        }
    }
    let pem = instance_key(state, key_id).await?;
    // 3. Outer HTTP: Host against the configured public origin, Date +-5 min, Digest of the bytes.
    if h.get("host").map(String::as_str) != Some(state.own_authority.as_str()) {
        return Err(reject(401, "host_mismatch"));
    }
    let date = h
        .get("date")
        .and_then(|d| httpdate::parse_http_date(d).ok())
        .ok_or(reject(401, "bad_date"))?;
    let skew = match std::time::SystemTime::now().duration_since(date) {
        Ok(d) => d,
        Err(e) => e.duration(),
    };
    if skew > Duration::from_secs(300) {
        return Err(reject(401, "date_out_of_range"));
    }
    let digest = format!(
        "SHA-256={}",
        base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            Sha256::digest(body)
        )
    );
    if h.get("digest") != Some(&digest) {
        return Err(reject(401, "digest_mismatch"));
    }
    let mut lines = vec![];
    for name in &covered {
        if *name == "(request-target)" {
            lines.push(format!(
                "(request-target): {} {}",
                method.to_lowercase(),
                target
            ));
        } else {
            lines.push(format!(
                "{name}: {}",
                h.get(*name).ok_or(reject(401, "covered_header_missing"))?
            ));
        }
    }
    let raw_sig = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        p.get("signature").ok_or(reject(401, "no_signature"))?,
    )
    .map_err(|_| reject(401, "bad_signature"))?;
    let public =
        rsa::RsaPublicKey::from_public_key_pem(&pem).map_err(|_| reject(401, "bad_key"))?;
    VerifyingKey::<Sha256>::new(public)
        .verify(
            lines.join("\n").as_bytes(),
            &Signature::try_from(raw_sig.as_slice()).map_err(|_| reject(401, "bad_signature"))?,
        )
        .map_err(|_| reject(401, "signature_invalid"))?;
    // 6 (P1 part). Size, version, schema.
    if body.len() > 2 * 1024 * 1024 {
        return Err(reject(413, "envelope_too_large"));
    }
    let envelope: Value = serde_json::from_slice(body).map_err(|_| reject(400, "not_json"))?;
    if envelope["version"] != 1 {
        return Err(reject(400, "unsupported_version"));
    }
    // The loopback receiver is addressed as http://127.0.0.1:PORT, which the HTTPS-only origin
    // pattern of the schema cannot express: validate with that single spelling substituted.
    let mut checked = envelope.clone();
    if checked["recipient"]["origin"] == state.own_origin {
        checked["recipient"]["origin"] = Value::String(format!("https://{}", state.own_authority));
    }
    if !state.validator.is_valid(&checked) {
        return Err(reject(400, "schema_violation"));
    }
    outside_schema_checks(&envelope)?;
    // 4. Sender origin / instance actor / Profile actor origin all belong to the trusted Emumet.
    let instance = format!("{}/ap/instance", state.trusted_origin);
    if envelope["sender"]["origin"] != state.trusted_origin
        || envelope["sender"]["instance_actor"] != instance
    {
        return Err(reject(403, "sender_mismatch"));
    }
    if origin_of(
        envelope["recipient"]["profile_actor"]
            .as_str()
            .unwrap_or_default(),
    )
    .as_deref()
        != Some(state.trusted_origin.as_str())
    {
        return Err(reject(403, "profile_origin_mismatch"));
    }
    // 5. Addressed to this receiver. link_id / link_generation are trusted claims: no ledger.
    if envelope["recipient"]["origin"] != state.own_origin {
        return Err(reject(403, "recipient_mismatch"));
    }
    let receipt = envelope["receipt_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    Ok((receipt, envelope))
}

const INBOX_BODY_MAX_BYTES: usize = 1_048_576;
const EVIDENCE_BUDGET_BYTES: usize = 16_384;

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        _ => 0,
    }
}

/// Contract 4.5 / 4.4 item 5: the day exists in that month and year. Only applied to strings
/// that already passed the schema pattern (`YYYY-MM-DDT...`).
fn calendar_day_exists(timestamp: &str) -> bool {
    let (Some(y), Some(m), Some(d)) = (
        timestamp.get(0..4),
        timestamp.get(5..7),
        timestamp.get(8..10),
    ) else {
        return false;
    };
    let (Ok(y), Ok(m), Ok(d)) = (y.parse::<i64>(), m.parse::<i64>(), d.parse::<i64>()) else {
        return false;
    };
    (1..=days_in_month(y, m)).contains(&d)
}

/// Contract 4.4 items 2, 3 and 5: the checks JSON Schema cannot express. Item 1 (the request
/// body size) and item 4 (cross-field rules) are performed by the caller.
fn outside_schema_checks(envelope: &Value) -> Result<(), Reject> {
    // 2. The decoded original body is at most INBOX_BODY_MAX_BYTES (the schema's Base64 character
    //    limit is slightly larger than the exact byte limit).
    let raw = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        envelope["activity"]["raw_body"]
            .as_str()
            .unwrap_or_default(),
    )
    .map_err(|_| reject(400, "schema_violation"))?;
    if raw.len() > INBOX_BODY_MAX_BYTES {
        return Err(reject(413, "activity_too_large"));
    }
    // 3. Retained evidence headers plus the omitted names stay within the budget.
    let kept: usize = envelope["original_request"]["signature_headers"]
        .as_array()
        .map(|headers| {
            headers
                .iter()
                .map(|h| {
                    h["name"].as_str().unwrap_or_default().len()
                        + h["value"].as_str().unwrap_or_default().len()
                })
                .sum()
        })
        .unwrap_or(0);
    let omitted: usize = envelope["omitted_header_names"]
        .as_array()
        .map(|names| {
            names
                .iter()
                .map(|n| n.as_str().unwrap_or_default().len())
                .sum()
        })
        .unwrap_or(0);
    if kept + omitted > EVIDENCE_BUDGET_BYTES {
        return Err(reject(400, "evidence_budget_exceeded"));
    }
    // 5. received_at / verified_at name an existing calendar date.
    for timestamp in [
        &envelope["received_at"],
        &envelope["verification"]["verified_at"],
    ] {
        if !calendar_day_exists(timestamp.as_str().unwrap_or_default()) {
            return Err(reject(400, "schema_violation"));
        }
    }
    Ok(())
}

async fn serve(state: Arc<State>, mut stream: TcpStream) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 16384];
    let head_end = loop {
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        if buf.len() > 64 * 1024 {
            return;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default().to_string();
    let mut parts = request_line.split(' ');
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let length: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[head_end..].to_vec();
    while body.len() < length {
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => body.extend_from_slice(&chunk[..n]),
        }
    }
    body.truncate(length);
    let behavior = state
        .script
        .lock()
        .unwrap()
        .pop_front()
        .unwrap_or_else(|| state.default.lock().unwrap().clone());
    let respond = |status: u16, extra: &str| {
        format!("HTTP/1.1 {status} X\r\ncontent-length: 0\r\nconnection: close\r\n{extra}\r\n")
    };
    let outcome;
    let mut reply: Option<String> = None;
    match behavior.clone() {
        Behavior::Status(code) => {
            outcome = format!("status {code}");
            reply = Some(respond(code, ""));
        }
        Behavior::Redirect(to) => {
            outcome = "redirect".into();
            reply = Some(respond(302, &format!("location: {to}\r\n")));
        }
        Behavior::Stall => {
            outcome = "stall".into();
            state.attempts.lock().unwrap().push(Attempt {
                body: body.clone(),
                headers: headers.clone(),
                outcome: outcome.clone(),
            });
            tokio::time::sleep(Duration::from_secs(300)).await;
            return;
        }
        Behavior::Accept | Behavior::AcceptThenDisconnect => {
            match verify(&state, &method, &target, &headers, &body).await {
                Ok((receipt, envelope)) => {
                    let first = state.seen.lock().unwrap().insert((
                        envelope["sender"]["origin"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                        receipt.clone(),
                    ));
                    if first {
                        let raw = base64::Engine::decode(
                            &base64::engine::general_purpose::STANDARD,
                            envelope["activity"]["raw_body"]
                                .as_str()
                                .unwrap_or_default(),
                        )
                        .unwrap_or_default();
                        state.accepted.lock().unwrap().push(Accepted {
                            receipt_id: receipt,
                            envelope,
                            outer_body: body.clone(),
                            raw_body: raw,
                            headers: headers.clone(),
                        });
                    }
                    if matches!(behavior, Behavior::AcceptThenDisconnect) {
                        outcome = "accepted-then-disconnect".into();
                    } else {
                        outcome = if first {
                            "accepted".into()
                        } else {
                            "duplicate".into()
                        };
                        reply = Some(respond(202, ""));
                    }
                }
                Err(r) => {
                    outcome = format!("rejected {}", r.code);
                    reply = Some(respond(r.status, ""));
                    state.rejected.lock().unwrap().push(r);
                }
            }
        }
    }
    state.attempts.lock().unwrap().push(Attempt {
        body,
        headers,
        outcome,
    });
    if let Some(reply) = reply {
        let _ = stream.write_all(reply.as_bytes()).await;
        let _ = stream.shutdown().await;
    }
}
