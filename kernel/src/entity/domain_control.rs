//! Profile x exact-match domain control (mute / block), D8P.
//!
//! A control belongs to one Profile and names one **normalized domain**. Matching is exact:
//! a control for `example.com` never applies to `sub.example.com`. The subject that is matched
//! against the stored domain is the host of the **verified canonical actor ID** (see
//! [`ControlDomain::from_actor_id`]); inbox, shared inbox, CDN and media hosts are never used.

use crate::entity::ProfileId;
use idna::uts46::{AsciiDenyList, DnsLength, Hyphens, Uts46};
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use time::OffsetDateTime;

/// Why an input could not be turned into a [`ControlDomain`].
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum InvalidDomain {
    Empty,
    /// Contains whitespace or a control character.
    Whitespace,
    /// A scheme, userinfo, path, query, fragment or port is present (`https://x`, `a/b`, `x:80`, `u@x`).
    NotAHostName,
    /// Wildcards (`*.example.com`) are not supported: matching is exact.
    Wildcard,
    /// IPv4 / IPv6 literals are not domains.
    IpAddress,
    /// IDNA (UTS #46) processing or DNS length verification failed.
    Idna,
    /// The normalized form contains a character outside `[a-z0-9_.-]`.
    InvalidCharacter,
    /// An actor ID that is not an absolute `http`/`https` URL with a DNS host name.
    InvalidActorId,
}

impl Display for InvalidDomain {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            InvalidDomain::Empty => "domain must not be empty",
            InvalidDomain::Whitespace => "domain must not contain whitespace or control characters",
            InvalidDomain::NotAHostName => {
                "domain must be a bare host name (no scheme, userinfo, port, path, query or fragment)"
            }
            InvalidDomain::Wildcard => "wildcards are not supported; domains match exactly",
            InvalidDomain::IpAddress => "IP addresses are not domains",
            InvalidDomain::Idna => "domain is not a valid IDNA host name",
            InvalidDomain::InvalidCharacter => "domain contains an unsupported character",
            InvalidDomain::InvalidActorId => {
                "actor ID must be an absolute http(s) URL without credentials and with a host name"
            }
        };
        f.write_str(text)
    }
}

impl std::error::Error for InvalidDomain {}

/// A normalized, exact-match domain (lowercase ASCII, IDNA A-labels, no trailing dot).
///
/// Normalization (applied by [`ControlDomain::parse`]):
/// 1. reject empty input, whitespace / control characters, `* : / ? # @ \ [ ] % ^ | < >`
///    (i.e. no scheme, userinfo, port, path, wildcard or IP literal);
/// 2. remove **one** trailing root dot (`example.com.` == `example.com`; `example.com..` is invalid);
/// 3. UTS #46 non-transitional mapping to ASCII: case folding, Unicode normalization,
///    full-width / ideographic dots as separators, Punycode (`bücher.example` ->
///    `xn--bcher-kva.example`), validation of existing `xn--` labels, DNS length limits
///    (label <= 63, name <= 253, no empty label);
/// 4. result limited to `[a-z0-9_-]` labels joined by `.` (underscore is tolerated because it
///    occurs in real host names);
/// 5. a purely numeric last label is rejected (IPv4-like).
#[derive(Debug, Clone, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct ControlDomain(String);

impl ControlDomain {
    /// Normalize user input (a domain typed by the owner). Idempotent on already normalized values.
    pub fn parse(input: &str) -> Result<Self, InvalidDomain> {
        if input.is_empty() {
            return Err(InvalidDomain::Empty);
        }
        if input.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(InvalidDomain::Whitespace);
        }
        if input.contains('*') {
            return Err(InvalidDomain::Wildcard);
        }
        if input.chars().any(|c| {
            matches!(
                c,
                ':' | '/' | '?' | '#' | '@' | '\\' | '[' | ']' | '%' | '^' | '|' | '<' | '>'
            )
        }) {
            return Err(InvalidDomain::NotAHostName);
        }
        let without_root_dot = input.strip_suffix('.').unwrap_or(input);
        if without_root_dot.is_empty() {
            return Err(InvalidDomain::Empty);
        }
        let ascii = Uts46::new()
            .to_ascii(
                without_root_dot.as_bytes(),
                AsciiDenyList::URL,
                Hyphens::Allow,
                DnsLength::Verify,
            )
            .map_err(|_| InvalidDomain::Idna)?;
        let normalized = ascii.into_owned();
        if !normalized
            .bytes()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.'))
        {
            return Err(InvalidDomain::InvalidCharacter);
        }
        let last_label = normalized.rsplit('.').next().unwrap_or_default();
        if last_label.is_empty() {
            return Err(InvalidDomain::Empty);
        }
        if last_label.bytes().all(|b| b.is_ascii_digit()) {
            return Err(InvalidDomain::IpAddress);
        }
        Ok(Self(normalized))
    }

    /// The matching key of a remote or local actor: the host of its **verified canonical actor ID**.
    ///
    /// Only the actor ID is accepted here. Callers must pass the ID they verified against the
    /// actor document, never an `inbox` / `sharedInbox` / `endpoints` / icon / media URL: those
    /// hosts (CDNs, shared inbox clusters) are not the actor's domain.
    /// The port is ignored (the domain is the host), credentials are rejected, and the host is
    /// normalized exactly like [`ControlDomain::parse`].
    pub fn from_actor_id(actor_id: &str) -> Result<Self, InvalidDomain> {
        let url = url::Url::parse(actor_id).map_err(|_| InvalidDomain::InvalidActorId)?;
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(InvalidDomain::InvalidActorId);
        }
        match url.host() {
            Some(url::Host::Domain(host)) => Self::parse(host),
            Some(_) => Err(InvalidDomain::IpAddress),
            None => Err(InvalidDomain::InvalidActorId),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl AsRef<str> for ControlDomain {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Display for ControlDomain {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DomainControlMode {
    /// Display suppression only; relationships are kept.
    Mute,
    /// Display suppression, bidirectional follow detachment and rejection of new follows.
    Block,
}

impl DomainControlMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            DomainControlMode::Mute => "mute",
            DomainControlMode::Block => "block",
        }
    }

    pub fn from_value(value: &str) -> Option<Self> {
        match value {
            "mute" => Some(DomainControlMode::Mute),
            "block" => Some(DomainControlMode::Block),
            _ => None,
        }
    }
}

/// One live control: Profile x normalized domain.
///
/// `revision` starts at 1 and increases on every change, **including delete and re-create**:
/// a deleted control keeps its revision counter, so a stale `expected_revision` can never match
/// a later incarnation (no ABA).
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DomainControl {
    profile_id: ProfileId,
    domain: ControlDomain,
    mode: DomainControlMode,
    revision: u64,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

impl DomainControl {
    pub fn new(
        profile_id: ProfileId,
        domain: ControlDomain,
        mode: DomainControlMode,
        revision: u64,
        created_at: OffsetDateTime,
        updated_at: OffsetDateTime,
    ) -> Self {
        Self {
            profile_id,
            domain,
            mode,
            revision,
            created_at,
            updated_at,
        }
    }

    pub fn profile_id(&self) -> &ProfileId {
        &self.profile_id
    }

    pub fn domain(&self) -> &ControlDomain {
        &self.domain
    }

    pub fn mode(&self) -> DomainControlMode {
        self.mode
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn created_at(&self) -> OffsetDateTime {
        self.created_at
    }

    pub fn updated_at(&self) -> OffsetDateTime {
        self.updated_at
    }

    /// Whether this control applies to the actor with the given **verified canonical actor ID**.
    /// Exact host match only; an unparsable actor ID never matches.
    pub fn applies_to_actor_id(&self, actor_id: &str) -> bool {
        ControlDomain::from_actor_id(actor_id).is_ok_and(|domain| domain == self.domain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Normalization table (input, expected normalized form). Fixed by D8P.
    const NORMALIZATION_TABLE: &[(&str, &str)] = &[
        ("example.com", "example.com"),
        ("EXAMPLE.COM", "example.com"),
        ("Example.Com", "example.com"),
        ("example.com.", "example.com"),
        ("EXAMPLE.COM.", "example.com"),
        ("sub.example.com", "sub.example.com"),
        ("a-b.example.com", "a-b.example.com"),
        ("foo_bar.example.com", "foo_bar.example.com"),
        ("xn--bcher-kva.example", "xn--bcher-kva.example"),
        ("XN--BCHER-KVA.EXAMPLE", "xn--bcher-kva.example"),
        ("bücher.example", "xn--bcher-kva.example"),
        ("BÜCHER.example", "xn--bcher-kva.example"),
        ("bu\u{0308}cher.example", "xn--bcher-kva.example"),
        ("bücher.example.", "xn--bcher-kva.example"),
        ("日本語.jp", "xn--wgv71a119e.jp"),
        ("例え.テスト", "xn--r8jz45g.xn--zckzah"),
        ("пример.рф", "xn--e1afmkfd.xn--p1ai"),
        // ideographic full stop is a label separator under UTS #46
        ("example\u{3002}com", "example.com"),
        // full-width Latin letters are mapped to ASCII
        ("ｅｘａｍｐｌｅ.com", "example.com"),
        // German sharp s is kept (non-transitional processing), not folded to "ss"
        ("faß.example", "xn--fa-hia.example"),
        ("localhost", "localhost"),
        ("127.example.com", "127.example.com"),
    ];

    #[test]
    fn normalization_table_is_fixed() {
        for (input, expected) in NORMALIZATION_TABLE {
            let parsed = ControlDomain::parse(input)
                .unwrap_or_else(|e| panic!("{input:?} must normalize but failed: {e}"));
            assert_eq!(parsed.as_str(), *expected, "input {input:?}");
        }
    }

    #[test]
    fn normalization_is_idempotent() {
        for (input, _) in NORMALIZATION_TABLE {
            let once = ControlDomain::parse(input).unwrap();
            let twice = ControlDomain::parse(once.as_str()).unwrap();
            assert_eq!(once, twice, "input {input:?}");
        }
    }

    #[test]
    fn equivalent_spellings_normalize_to_one_domain() {
        let canonical = ControlDomain::parse("bücher.example").unwrap();
        for spelling in [
            "BÜCHER.EXAMPLE",
            "xn--bcher-kva.example",
            "Xn--Bcher-Kva.Example.",
            "bu\u{0308}cher.example",
        ] {
            assert_eq!(ControlDomain::parse(spelling).unwrap(), canonical);
        }
    }

    /// Rejection table (input, expected reason). Fixed by D8P.
    const REJECTION_TABLE: &[(&str, InvalidDomain)] = &[
        ("", InvalidDomain::Empty),
        (".", InvalidDomain::Empty),
        (" example.com", InvalidDomain::Whitespace),
        ("example.com ", InvalidDomain::Whitespace),
        ("exa mple.com", InvalidDomain::Whitespace),
        ("example.com\n", InvalidDomain::Whitespace),
        ("example.com\t", InvalidDomain::Whitespace),
        ("*.example.com", InvalidDomain::Wildcard),
        ("*", InvalidDomain::Wildcard),
        ("https://example.com", InvalidDomain::NotAHostName),
        ("example.com/path", InvalidDomain::NotAHostName),
        ("example.com:8080", InvalidDomain::NotAHostName),
        ("user@example.com", InvalidDomain::NotAHostName),
        ("example.com?x=1", InvalidDomain::NotAHostName),
        ("example.com#frag", InvalidDomain::NotAHostName),
        ("example.com\\x", InvalidDomain::NotAHostName),
        ("ex%61mple.com", InvalidDomain::NotAHostName),
        ("[::1]", InvalidDomain::NotAHostName),
        ("::1", InvalidDomain::NotAHostName),
        ("127.0.0.1", InvalidDomain::IpAddress),
        ("192.168.1.1.", InvalidDomain::IpAddress),
        ("example.com..", InvalidDomain::Idna),
        (".example.com", InvalidDomain::Idna),
        ("a..example.com", InvalidDomain::Idna),
        ("xn--.example", InvalidDomain::Idna),
        ("xn--abc-.example", InvalidDomain::Idna),
        ("xn--a.example", InvalidDomain::Idna),
        ("exa!mple.com", InvalidDomain::InvalidCharacter),
        ("exa$mple.com", InvalidDomain::InvalidCharacter),
        ("exa,mple.com", InvalidDomain::InvalidCharacter),
    ];

    #[test]
    fn rejection_table_is_fixed() {
        for (input, expected) in REJECTION_TABLE {
            let result = ControlDomain::parse(input);
            assert_eq!(
                result.as_ref().err(),
                Some(expected),
                "input {input:?} -> {result:?}"
            );
        }
    }

    #[test]
    fn length_limits_are_enforced() {
        let label63 = "a".repeat(63);
        assert!(ControlDomain::parse(&format!("{label63}.example")).is_ok());
        let label64 = "a".repeat(64);
        assert_eq!(
            ControlDomain::parse(&format!("{label64}.example")),
            Err(InvalidDomain::Idna)
        );
        // 253 characters is the DNS maximum, 254 is rejected.
        let ok = [label63.as_str(); 3].join(".") + "." + &"b".repeat(61);
        assert_eq!(ok.len(), 253);
        assert!(ControlDomain::parse(&ok).is_ok());
        let too_long = format!("{ok}b");
        assert_eq!(ControlDomain::parse(&too_long), Err(InvalidDomain::Idna));
    }

    #[test]
    fn exact_match_does_not_extend_to_subdomains_or_parents() {
        let parent = ControlDomain::parse("example.com").unwrap();
        let child = ControlDomain::parse("sub.example.com").unwrap();
        assert_ne!(parent, child);
        let control = DomainControl::new(
            ProfileId::new(1),
            parent,
            DomainControlMode::Block,
            1,
            OffsetDateTime::UNIX_EPOCH,
            OffsetDateTime::UNIX_EPOCH,
        );
        assert!(control.applies_to_actor_id("https://example.com/users/alice"));
        assert!(control.applies_to_actor_id("https://EXAMPLE.com:8443/users/alice"));
        assert!(control.applies_to_actor_id("https://example.com./users/alice"));
        assert!(!control.applies_to_actor_id("https://sub.example.com/users/alice"));
        assert!(!control.applies_to_actor_id("https://notexample.com/users/alice"));
        assert!(!control.applies_to_actor_id("https://example.com.evil.test/users/alice"));
        let child_control = DomainControl::new(
            ProfileId::new(1),
            child,
            DomainControlMode::Mute,
            1,
            OffsetDateTime::UNIX_EPOCH,
            OffsetDateTime::UNIX_EPOCH,
        );
        assert!(!child_control.applies_to_actor_id("https://example.com/users/alice"));
    }

    /// Actor-ID host extraction table (input, expected domain).
    const ACTOR_ID_TABLE: &[(&str, &str)] = &[
        ("https://example.com/users/alice", "example.com"),
        ("https://Example.COM/users/alice", "example.com"),
        ("https://example.com:8443/users/alice", "example.com"),
        ("http://example.com/actor", "example.com"),
        ("https://example.com./users/alice", "example.com"),
        (
            "https://bücher.example/users/alice",
            "xn--bcher-kva.example",
        ),
        (
            "https://xn--bcher-kva.example/users/alice",
            "xn--bcher-kva.example",
        ),
        ("https://sub.example.com/@bob?x=1#frag", "sub.example.com"),
    ];

    #[test]
    fn actor_id_host_is_the_matching_key() {
        for (actor_id, expected) in ACTOR_ID_TABLE {
            assert_eq!(
                ControlDomain::from_actor_id(actor_id).unwrap().as_str(),
                *expected,
                "actor id {actor_id:?}"
            );
        }
    }

    #[test]
    fn actor_id_with_trick_hosts_does_not_resolve_to_the_decoy() {
        // credentials are rejected outright, so `good.example` cannot be smuggled into the userinfo
        assert_eq!(
            ControlDomain::from_actor_id("https://good.example@evil.test/users/a"),
            Err(InvalidDomain::InvalidActorId)
        );
        assert_eq!(
            ControlDomain::from_actor_id("https://user:pw@evil.test/users/a"),
            Err(InvalidDomain::InvalidActorId)
        );
        // the host is what follows the userinfo-less authority
        assert_eq!(
            ControlDomain::from_actor_id("https://evil.test/good.example")
                .unwrap()
                .as_str(),
            "evil.test"
        );
    }

    #[test]
    fn invalid_actor_ids_are_rejected() {
        for actor_id in [
            "",
            "example.com",
            "/users/alice",
            "ftp://example.com/users/alice",
            "acct:alice@example.com",
            "https://",
            "https://127.0.0.1/users/alice",
            "https://[::1]/users/alice",
            "https://192.168.0.1:8080/users/alice",
        ] {
            assert!(
                ControlDomain::from_actor_id(actor_id).is_err(),
                "{actor_id:?} must be rejected"
            );
        }
    }

    #[test]
    fn unparsable_actor_id_never_matches() {
        let control = DomainControl::new(
            ProfileId::new(1),
            ControlDomain::parse("example.com").unwrap(),
            DomainControlMode::Block,
            1,
            OffsetDateTime::UNIX_EPOCH,
            OffsetDateTime::UNIX_EPOCH,
        );
        assert!(!control.applies_to_actor_id("not a url"));
        assert!(!control.applies_to_actor_id("https://127.0.0.1/users/a"));
    }

    #[test]
    fn mode_round_trips_through_its_stored_value() {
        for mode in [DomainControlMode::Mute, DomainControlMode::Block] {
            assert_eq!(DomainControlMode::from_value(mode.as_str()), Some(mode));
        }
        assert_eq!(DomainControlMode::from_value("ban"), None);
    }
}
