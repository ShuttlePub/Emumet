use driver::registration_protection::{ProcessEnv, ValueSource};
use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};

/// Deliberately not Debug: contains the shared authentication secret.
pub struct GuardConfig {
    pub listen: SocketAddr,
    pub allowed_peers: Vec<IpAddr>,
    pub secret: String,
    pub deadline: Duration,
    pub period: Duration,
}

impl GuardConfig {
    pub fn from_env() -> Result<Option<Self>, &'static str> {
        Self::from_source(&ProcessEnv)
    }

    pub(super) fn from_source(source: &dyn ValueSource) -> Result<Option<Self>, &'static str> {
        match source.get("REGISTRATION_ENFORCEMENT_ENABLED").as_deref() {
            None | Some("false") => return Ok(None),
            Some("true") => {}
            _ => return Err("REGISTRATION_ENFORCEMENT_ENABLED must be true or false"),
        }
        let secret = match (
            source.get("REGISTRATION_HOOK_SECRET"),
            source.get("REGISTRATION_HOOK_SECRET_FILE"),
        ) {
            (Some(secret), None) => secret,
            (None, Some(path)) => std::fs::read_to_string(&path)
                .map_err(|_| "Cannot read registration hook secret")?,
            _ => return Err("Set exactly one registration hook secret source"),
        };
        let secret = secret.trim().to_string();
        if secret.len() < 32 || !secret.bytes().all(|b| b.is_ascii_graphic()) {
            return Err("Registration hook secret must contain at least 32 visible ASCII bytes");
        }
        let listen = source
            .get("REGISTRATION_HOOK_LISTEN")
            .unwrap_or_else(|| "127.0.0.1:8081".into())
            .parse()
            .map_err(|_| "Invalid registration hook listen address")?;
        let peers = source
            .get("REGISTRATION_HOOK_ALLOWED_PEERS")
            .unwrap_or_else(|| "127.0.0.1,::1".into());
        let allowed_peers: Vec<IpAddr> = peers
            .split(',')
            .map(|v| v.trim().parse())
            .collect::<Result<_, _>>()
            .map_err(|_| "Invalid registration hook peer allowlist")?;
        if allowed_peers.is_empty() {
            return Err("Registration hook peer allowlist cannot be empty");
        }
        let secs: u64 = source
            .get("REGISTRATION_PROTECTION_POLL_SECS")
            .unwrap_or_else(|| "30".into())
            .parse()
            .map_err(|_| "Invalid registration maintenance period")?;
        if secs == 0 {
            return Err("Registration maintenance period must be positive");
        }
        Ok(Some(Self {
            listen,
            allowed_peers,
            secret,
            deadline: Duration::from_secs(5),
            period: Duration::from_secs(secs),
        }))
    }
}
