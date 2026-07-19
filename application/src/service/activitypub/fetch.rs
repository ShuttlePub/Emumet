use error_stack::Report;
use kernel::KernelError;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

pub(super) async fn validate_fetch_url(
    url: &reqwest::Url,
) -> error_stack::Result<Vec<SocketAddr>, KernelError> {
    match url.scheme() {
        "http" | "https" => {}
        scheme => {
            return Err(Report::new(KernelError::Rejected)
                .attach_printable(format!("SsrfBlocked: unsupported URL scheme '{scheme}'")));
        }
    }

    if !url.username().is_empty() || url.password().is_some() {
        return Err(Report::new(KernelError::Rejected)
            .attach_printable("SsrfBlocked: URL credentials are not allowed"));
    }

    let host = url.host_str().ok_or_else(|| {
        Report::new(KernelError::Rejected).attach_printable("SsrfBlocked: URL host is empty")
    })?;
    let host_lc = host.trim_end_matches('.').to_ascii_lowercase();

    if cfg!(not(any(test, feature = "test-mode"))) {
        if !is_fetch_host_allowed(&host_lc)
            && (host_lc == "localhost" || host_lc.ends_with(".localhost"))
        {
            return Err(Report::new(KernelError::Rejected)
                .attach_printable("SsrfBlocked: localhost URL is not allowed"));
        }
    }

    let port = url.port_or_known_default().ok_or_else(|| {
        Report::new(KernelError::Rejected).attach_printable("SsrfBlocked: URL has no usable port")
    })?;
    if let Ok(ip) = host_lc.parse::<IpAddr>() {
        if cfg!(not(any(test, feature = "test-mode"))) {
            validate_public_ip(ip)?;
        }
        return Ok(vec![SocketAddr::new(ip, port)]);
    }

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
    if cfg!(not(any(test, feature = "test-mode"))) {
        for address in &addresses {
            validate_public_ip(address.ip())?;
        }
    }
    Ok(addresses)
}

pub(super) fn client_for_url(
    url: &reqwest::Url,
    resolved_addresses: &[SocketAddr],
) -> error_stack::Result<reqwest::Client, KernelError> {
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(10));
    #[cfg(any(test, feature = "test-mode"))]
    if std::env::var("AP_TEST_ACCEPT_INVALID_CERTS").as_deref() == Ok("1") {
        builder = builder.danger_accept_invalid_certs(true);
    }
    if let Some(host) = url.host_str() {
        if host.parse::<IpAddr>().is_err() {
            builder = builder.resolve_to_addrs(host, resolved_addresses);
        }
    }
    builder.build().map_err(|e| {
        Report::new(KernelError::Internal)
            .attach_printable(format!("Failed to build pinned HTTP client: {e}"))
    })
}

/// Checks whether a host is allowlisted for test-mode AP fetch operations.
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

fn validate_public_ip(ip: IpAddr) -> error_stack::Result<(), KernelError> {
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
        || (ip.segments()[0] & 0xffff) == 0x2001 && (ip.segments()[1] & 0xfff0) == 0x0db8
        || ip.segments()[0] == 0x2002
        || (ip.segments()[0] == 0x2001 && ip.segments()[1] == 0)
}
