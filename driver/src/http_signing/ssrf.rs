use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use error_stack::{Report, Result};
use kernel::KernelError;

/// Checks whether a host is allowlisted for test-mode AP key fetch operations.
///
/// Reads the `AP_TEST_ALLOWED_FETCH_HOSTS` environment variable (comma-separated,
/// trimmed, lowercase) and returns true if `host_lc` matches any entry.
/// Returns false when the env var is unset or empty.
fn is_fetch_host_allowed(host_lc: &str) -> bool {
    std::env::var("AP_TEST_ALLOWED_FETCH_HOSTS")
        .ok()
        .is_some_and(|val| {
            val.split(',')
                .any(|entry| entry.trim().eq_ignore_ascii_case(host_lc))
        })
}

pub(super) async fn validate_fetch_url(url: &reqwest::Url) -> Result<Vec<SocketAddr>, KernelError> {
    match url.scheme() {
        "http" | "https" => {}
        scheme => {
            return Err(Report::new(KernelError::Rejected).attach_printable(format!(
                "SsrfBlocked: unsupported keyId URL scheme '{scheme}'"
            )));
        }
    }

    if !url.username().is_empty() || url.password().is_some() {
        return Err(Report::new(KernelError::Rejected)
            .attach_printable("SsrfBlocked: keyId URL credentials are not allowed"));
    }

    let host = url.host_str().ok_or_else(|| {
        Report::new(KernelError::Rejected).attach_printable("SsrfBlocked: keyId URL host is empty")
    })?;
    let host_lc = host.trim_end_matches('.').to_ascii_lowercase();

    let ssrf_bypassed = cfg!(any(test, feature = "test-mode")) && is_fetch_host_allowed(&host_lc);

    if !ssrf_bypassed {
        if host_lc == "localhost" || host_lc.ends_with(".localhost") {
            return Err(Report::new(KernelError::Rejected)
                .attach_printable("SsrfBlocked: localhost keyId URL is not allowed"));
        }

        if let Ok(ip) = host_lc.parse::<IpAddr>() {
            validate_public_ip(ip)?;
            let port = url.port_or_known_default().ok_or_else(|| {
                Report::new(KernelError::Rejected)
                    .attach_printable("SsrfBlocked: keyId URL does not have a usable port")
            })?;
            return Ok(vec![SocketAddr::new(ip, port)]);
        }

        let port = url.port_or_known_default().ok_or_else(|| {
            Report::new(KernelError::Rejected)
                .attach_printable("SsrfBlocked: keyId URL does not have a usable port")
        })?;
        let addresses = tokio::net::lookup_host((host_lc.as_str(), port))
            .await
            .map_err(|e| {
                Report::new(KernelError::Rejected)
                    .attach_printable(format!("SsrfBlocked: DNS resolution failed: {e}"))
            })?
            .collect::<Vec<_>>();

        if addresses.is_empty() {
            return Err(Report::new(KernelError::Rejected)
                .attach_printable("SsrfBlocked: DNS resolution returned no addresses"));
        }

        for address in &addresses {
            validate_public_ip(address.ip())?;
        }

        Ok(addresses)
    } else {
        let port = url.port_or_known_default().ok_or_else(|| {
            Report::new(KernelError::Rejected)
                .attach_printable("SsrfBlocked: keyId URL does not have a usable port")
        })?;
        let addresses = tokio::net::lookup_host((host_lc.as_str(), port))
            .await
            .map_err(|e| {
                Report::new(KernelError::Rejected)
                    .attach_printable(format!("SsrfBlocked: DNS resolution failed: {e}"))
            })?
            .collect::<Vec<_>>();

        if addresses.is_empty() {
            return Err(Report::new(KernelError::Rejected)
                .attach_printable("SsrfBlocked: DNS resolution returned no addresses"));
        }
        Ok(addresses)
    }
}

fn validate_public_ip(ip: IpAddr) -> Result<(), KernelError> {
    let blocked = match ip {
        IpAddr::V4(ip) => is_blocked_ipv4(ip),
        IpAddr::V6(ip) => is_blocked_ipv6(ip),
    };

    if blocked {
        Err(Report::new(KernelError::Rejected).attach_printable(format!(
            "SsrfBlocked: non-public IP address is not allowed: {ip}"
        )))
    } else {
        Ok(())
    }
}

fn is_blocked_ipv4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_multicast()
        || ip.is_unspecified()
        || octets[0] == 0
        || octets[0] >= 224
        || (octets[0] == 100 && (64..=127).contains(&octets[1]))
        || (octets[0] == 198 && (18..=19).contains(&octets[1]))
        || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
}

fn is_blocked_ipv6(ip: Ipv6Addr) -> bool {
    if let Some(ipv4) = ip.to_ipv4_mapped() {
        return is_blocked_ipv4(ipv4);
    }

    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (ip.segments()[0] & 0xfe00) == 0xfc00
        || (ip.segments()[0] & 0xffc0) == 0xfe80
        || (ip.segments()[0] == 0x2001 && ip.segments()[1] == 0x0db8)
        || ip.segments()[0] == 0x2002
        || (ip.segments()[0] == 0x2001 && ip.segments()[1] == 0)
}

#[cfg(test)]
mod tests {
    use super::is_blocked_ipv6;
    use std::net::Ipv6Addr;

    #[test]
    fn documentation_range_is_blocked() {
        let ip: Ipv6Addr = "2001:db8::1".parse().unwrap();
        assert!(is_blocked_ipv6(ip));
        let ip: Ipv6Addr = "2001:db8:ffff::1".parse().unwrap();
        assert!(is_blocked_ipv6(ip));
    }
}
