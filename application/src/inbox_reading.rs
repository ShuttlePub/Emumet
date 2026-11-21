//! Bounded, non-JSON-LD inbox reading and original-request evidence (contract v1 revision 3).
//! No network access; raw bytes are never reserialized for forwarding.
use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::{BTreeSet, HashSet};

pub const INBOX_BODY_MAX_BYTES: usize = 1_048_576;
pub const REQUEST_TARGET_MAX: usize = 2048;
fn constant(name: &str) -> u64 {
    match name {
        "URL_MAX" => 2048,
        "TYPE_STRING_MAX" => 512,
        "TYPE_ARRAY_MAX" | "ACTOR_ARRAY_MAX" => 16,
        "EVIDENCE_BUDGET_BYTES" | "EVIDENCE_HEADER_VALUE_MAX" => 16384,
        "EVIDENCE_MAX_HEADERS" => 64,
        "EVIDENCE_HEADER_NAME_MAX" => 128,
        "OMITTED_NAMES_MAX" => 32,
        "OMITTED_COUNT_MAX" => 1024,
        _ => unreachable!("unknown contract constant"),
    }
}
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
pub fn as_term(t: &str) -> Option<&str> {
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

pub fn type_strings(v: &Value) -> Option<Vec<String>> {
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

pub fn media_type_ok(ct: &str) -> bool {
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
    // One or more DNS labels separated by single dots; a label is letters, digits and hyphens and
    // neither starts nor ends with a hyphen (consecutive hyphens are fine: Punycode `xn--`).
    let host_ok = host.split('.').all(|label| {
        let c: Vec<char> = label.chars().collect();
        !c.is_empty()
            && alnum(c[0])
            && alnum(c[c.len() - 1])
            && c.iter().all(|&x| alnum(x) || x == '-')
    });
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

pub fn valid_sender_origin(value: &str) -> bool {
    value.len() <= 255
        && url_parts(value, false, true)
            .is_some_and(|(_, port, tail)| tail.is_empty() && port != Some(443))
}

pub fn is_remote_url(value: &str) -> bool {
    url_parts(value, true, false).is_some()
}

/// Static error codes only: never put malformed body fragments into errors or logs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InboxReadError {
    pub status: u16,
    pub reason: &'static str,
}
fn bad(reason: &'static str) -> InboxReadError {
    InboxReadError {
        status: 400,
        reason,
    }
}
/// No Debug: contains the peer's body.
pub struct InboxReading {
    pub value: Value,
    pub actor: String,
    pub activity_id: Option<String>,
    pub relation: Option<String>,
}
impl InboxReading {
    pub fn references(&self) -> Vec<&Value> {
        match self.value.get("object") {
            None => vec![],
            Some(Value::Array(a)) => a.iter().collect(),
            Some(v) => vec![v],
        }
    }
    /// Embedded Follow/Block is evidence; an id needs a transactional repository lookup.
    pub fn embedded_relation(reference: &Value) -> bool {
        reference
            .get("type")
            .and_then(type_strings)
            .is_some_and(|ts| {
                ts.iter()
                    .filter_map(|t| as_term(t))
                    .any(|t| matches!(t, "Follow" | "Block"))
            })
    }
    pub fn excluded_by(&self, mut known: impl FnMut(&Value) -> bool) -> bool {
        match self.relation.as_deref() {
            Some("Follow" | "Block") => true,
            Some(_) => {
                let refs = self.references();
                !refs.is_empty()
                    && refs
                        .into_iter()
                        .all(|r| Self::embedded_relation(r) || known(r))
            }
            None => false,
        }
    }
    /// Only the relation subsystem uses the strict typed model. to/cc never gate relay admission.
    pub fn relation_activity(&self) -> kernel::activitypub::Activity {
        kernel::activitypub::Activity {
            context: self.value.get("@context").cloned(),
            id: self.activity_id.clone().unwrap_or_default(),
            type_: self.relation.clone().unwrap_or_default(),
            actor: self.actor.clone(),
            object: self.value.get("object").cloned(),
            target: self.value.get("target").cloned(),
            to: None,
            cc: None,
        }
    }
}
pub fn read_inbox(body: &[u8], content_type: &str) -> Result<InboxReading, InboxReadError> {
    if body.len() > INBOX_BODY_MAX_BYTES {
        return Err(InboxReadError {
            status: 413,
            reason: "body_too_large",
        });
    }
    if !media_type_ok(content_type) {
        return Err(bad("invalid_content_type"));
    }
    let parsed = match serde_json::from_slice::<Strict>(body) {
        Ok(Strict(v)) => v,
        Err(e) if e.to_string().starts_with("duplicate_key") => return Err(bad("duplicate_key")),
        Err(_) => return Err(bad("invalid_json")),
    };
    let obj = parsed.as_object().ok_or_else(|| bad("not_object"))?;
    if depth_of(&parsed) > 32 {
        return Err(bad("too_deep"));
    }
    if nodes_of(&parsed) > 100_000 {
        return Err(bad("too_many_nodes"));
    }
    let types = obj
        .get("type")
        .and_then(type_strings)
        .ok_or_else(|| bad("invalid_type"))?;
    let actor = match obj.get("actor") {
        Some(Value::Array(a)) if !a.is_empty() && a.len() <= 16 => {
            let ids: Option<HashSet<String>> = a.iter().map(actor_id).collect();
            ids.filter(|s| s.len() == 1)
                .and_then(|s| s.into_iter().next())
        }
        Some(Value::Array(_)) | None => None,
        Some(v) => actor_id(v),
    }
    .ok_or_else(|| bad("actor_missing_or_ambiguous"))?;
    if !is_remote_url(&actor) {
        return Err(bad("invalid_actor"));
    }
    let relations: BTreeSet<&str> = types
        .iter()
        .filter_map(|t| as_term(t))
        .filter(|t| matches!(*t, "Follow" | "Block" | "Undo" | "Accept" | "Reject"))
        .collect();
    if relations.len() > 1 {
        return Err(bad("ambiguous_relation"));
    }
    let relation = relations.iter().next().map(|r| r.to_string());
    let activity_id = obj
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| is_remote_url(id))
        .map(str::to_string);
    Ok(InboxReading {
        value: parsed,
        actor,
        activity_id,
        relation,
    })
}
fn is_secret_header(name: &str) -> bool {
    matches!(
        name,
        "authorization" | "proxy-authorization" | "cookie" | "set-cookie"
    ) || ["token", "secret", "api-key", "apikey", "password"]
        .iter()
        .any(|w| name.contains(w))
}

/// An RFC 8941 string starting at `c[i] == '"'`: its unescaped value and the index after the closing
/// quote, or `None` when it is not terminated.
fn read_sf_string(c: &[char], mut i: usize) -> Option<(String, usize)> {
    let mut out = String::new();
    i += 1;
    while i < c.len() {
        match c[i] {
            '\\' => {
                out.push(*c.get(i + 1)?);
                i += 2;
            }
            '"' => return Some((out, i + 1)),
            ch => {
                out.push(ch);
                i += 1;
            }
        }
    }
    None
}

/// Header names covered by the signature fields (v1.md 4.3 group 2), pseudo-components skipped.
fn covered_names(fields: &[(String, String)]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for (_, v) in fields.iter().filter(|(n, _)| n == "signature") {
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
            if c[i] == '"' {
                i = read_sf_string(&c, i).map_or(c.len(), |(_, next)| next);
                continue;
            }
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
                let Some((name, next)) = read_sf_string(&c, i) else {
                    i = c.len();
                    break;
                };
                names.push(name.to_ascii_lowercase());
                i = next;
                while i < c.len() && c[i] != ' ' && c[i] != ')' {
                    if c[i] == '"' {
                        i = read_sf_string(&c, i).map_or(c.len(), |(_, next)| next);
                    } else {
                        i += 1;
                    }
                }
            }
        }
    }
    names
        .into_iter()
        .filter(|n| !n.starts_with('(') && !n.starts_with('@'))
        .collect()
}

/// Contract 4.3. HeaderMap order is stable per field; repeated fields remain separate.
pub fn build_evidence(original: &[(String, String)]) -> Value {
    let budget = 16384;
    let fields: Vec<_> = original
        .iter()
        .map(|(n, v)| (n.to_ascii_lowercase(), v.clone()))
        .collect();
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
