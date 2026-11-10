use crate::KernelError;
use error_stack::Report;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};

const MAX_URL_LEN: usize = 1024;

/// ActivityPub が URL として扱う識別子の最低限の検査。正規化 (大文字小文字・末尾 `/`・fragment 等) は
/// しない。canonical 形は呼び出し側 (P1 の URL 規則) が決める。保存層は **渡された文字列と完全一致**で扱う。
fn validate_http_url(kind: &str, value: &str) -> error_stack::Result<(), KernelError> {
    let invalid = |reason: &str| {
        Report::new(KernelError::Validation).attach_printable(format!("invalid {kind}: {reason}"))
    };
    if value.len() > MAX_URL_LEN {
        return Err(invalid("too long"));
    }
    if value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(invalid("contains whitespace or control characters"));
    }
    let rest = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
        .ok_or_else(|| invalid("must start with http:// or https://"))?;
    if rest.is_empty() || rest.starts_with('/') {
        return Err(invalid("missing host"));
    }
    Ok(())
}

macro_rules! url_newtype {
    ($(#[$meta:meta])* $name:ident, $kind:literal) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> error_stack::Result<Self, KernelError> {
                let value = value.into();
                validate_http_url($kind, &value)?;
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl Display for $name {
            fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl TryFrom<String> for $name {
            type Error = String;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value).map_err(|report| report.to_string())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::new(value).map_err(serde::de::Error::custom)
            }
        }
    };
}

url_newtype!(
    /// リモート actor の canonical actor URL (ActivityPub actor ID)。
    GraphActorUrl,
    "actor url"
);
url_newtype!(
    /// Activity の ID (URL)。Follow の世代・重複受信・配送 job の冪等性キーに使う。
    GraphActivityId,
    "activity id"
);
url_newtype!(
    /// 配送先 inbox の URL。
    GraphInboxUrl,
    "inbox url"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_http_and_https_urls() {
        assert!(GraphActorUrl::new("https://remote.example/users/bob").is_ok());
        assert!(GraphActivityId::new("http://localhost:3000/activities/1").is_ok());
        assert!(GraphInboxUrl::new("https://remote.example/inbox").is_ok());
    }

    #[test]
    fn rejects_malformed_urls() {
        for bad in [
            "",
            "remote.example/users/bob",
            "ftp://remote.example/users/bob",
            "https://",
            "https:///path",
            "https://remote.example/users/bob smith",
            "https://remote.example/\nusers",
        ] {
            let error = GraphActorUrl::new(bad).unwrap_err();
            assert_eq!(error.current_context(), &KernelError::Validation, "{bad:?}");
        }
        let too_long = format!("https://remote.example/{}", "a".repeat(MAX_URL_LEN));
        assert!(GraphActivityId::new(too_long).is_err());
    }

    #[test]
    fn does_not_normalize() {
        let a = GraphActorUrl::new("https://Remote.example/users/bob").unwrap();
        let b = GraphActorUrl::new("https://remote.example/users/bob").unwrap();
        assert_ne!(a, b);
        assert_eq!(a.as_str(), "https://Remote.example/users/bob");
    }

    #[test]
    fn deserialization_validates() {
        let ok: GraphActorUrl = serde_json::from_str("\"https://remote.example/u\"").unwrap();
        assert_eq!(ok.as_str(), "https://remote.example/u");
        assert!(serde_json::from_str::<GraphActorUrl>("\"not a url\"").is_err());
        assert_eq!(
            serde_json::to_string(&ok).unwrap(),
            "\"https://remote.example/u\""
        );
    }
}
