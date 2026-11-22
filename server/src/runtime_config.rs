//! Bounds apply to Ory authentication acquisition and the entire serving shutdown, respectively.
use kernel::KernelError;
use std::time::Duration;

pub(crate) struct RuntimeLimits {
    pub jwks_timeout: Duration,
    pub shutdown_timeout: Duration,
}
impl RuntimeLimits {
    pub fn from_env() -> error_stack::Result<Self, KernelError> {
        Ok(Self {
            jwks_timeout: Duration::from_millis(value("JWKS_TIMEOUT_MS", 3_000, 60_000)?),
            shutdown_timeout: Duration::from_secs(value("SERVER_SHUTDOWN_TIMEOUT_SECS", 25, 300)?),
        })
    }
}
fn parse(raw: Option<&str>, default: u64, maximum: u64) -> Option<u64> {
    match raw {
        None => Some(default),
        Some(raw) => raw
            .parse::<u64>()
            .ok()
            .filter(|v| (1..=maximum).contains(v)),
    }
}
fn value(key: &'static str, default: u64, maximum: u64) -> error_stack::Result<u64, KernelError> {
    let raw = dotenvy::var(key).ok();
    parse(raw.as_deref(), default, maximum).ok_or_else(|| {
        error_stack::Report::new(KernelError::Validation)
            .attach_printable(format!("{key} must be an integer in 1..={maximum}"))
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timeout_bounds_are_positive_finite_and_strict() {
        assert_eq!(parse(None, 3_000, 60_000), Some(3_000));
        assert_eq!(parse(Some("1"), 3_000, 60_000), Some(1));
        assert_eq!(parse(Some("60000"), 3_000, 60_000), Some(60_000));
        for raw in ["0", "60001", "-1", "", "abc", "18446744073709551616"] {
            assert_eq!(parse(Some(raw), 3_000, 60_000), None, "{raw}");
        }
        assert_eq!(parse(Some("300"), 25, 300), Some(300));
        assert_eq!(parse(Some("301"), 25, 300), None);
    }
}
