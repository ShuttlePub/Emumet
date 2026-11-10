use axum::http::HeaderMap;
use std::collections::HashSet;
use std::net::IpAddr;

const HEADER: &str = "x-forwarded-for";

pub(super) enum SourceError {
    Configuration,
    ForwardedChain,
}

fn normalize(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(ip) => ip.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip.into()),
        ip => ip,
    }
}

fn trusted_proxies(raw: &str) -> Result<HashSet<IpAddr>, SourceError> {
    if raw.trim().is_empty() {
        return Ok(HashSet::new());
    }
    let addresses: Vec<_> = raw.split(',').collect();
    if addresses.len() > 32 {
        return Err(SourceError::Configuration);
    }
    addresses
        .into_iter()
        .map(|value| {
            value
                .trim()
                .parse()
                .map(normalize)
                .map_err(|_| SourceError::Configuration)
        })
        .collect()
}

pub(super) fn from_request(
    peer: Option<IpAddr>,
    headers: &HeaderMap,
) -> Result<String, SourceError> {
    let raw = match std::env::var("REGISTRATION_EMAIL_TRUSTED_PROXY_IPS") {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => String::new(),
        Err(_) => return Err(SourceError::Configuration),
    };
    source(peer, headers, &trusted_proxies(&raw)?)
}

fn source(
    peer: Option<IpAddr>,
    headers: &HeaderMap,
    trusted: &HashSet<IpAddr>,
) -> Result<String, SourceError> {
    let Some(peer) = peer.map(normalize) else {
        // A missing transport address never creates an attacker-controlled bucket.
        return Ok("unknown".to_string());
    };
    let ip = if trusted.contains(&peer) {
        let mut values = headers.get_all(HEADER).iter();
        let value = values.next().ok_or(SourceError::ForwardedChain)?;
        if values.next().is_some() || value.len() > 1024 {
            return Err(SourceError::ForwardedChain);
        }
        let value = value.to_str().map_err(|_| SourceError::ForwardedChain)?;
        let entries: Vec<_> = value.split(',').collect();
        if entries.is_empty() || entries.len() > 16 {
            return Err(SourceError::ForwardedChain);
        }
        let chain: Vec<IpAddr> = entries
            .into_iter()
            .map(|v| {
                v.trim()
                    .parse()
                    .map(normalize)
                    .map_err(|_| SourceError::ForwardedChain)
            })
            .collect::<Result<_, _>>()?;
        // Peel only the explicitly trusted suffix. A client-supplied prefix is
        // not authoritative; every trusted proxy must append its observed peer.
        chain
            .iter()
            .rev()
            .copied()
            .find(|ip| !trusted.contains(ip))
            .unwrap_or(chain[0])
    } else {
        peer
    };
    Ok(match ip {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => {
            let s = ip.segments();
            format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn resolve(peer: &str, forwarded: &[&str], trusted: &str) -> Result<String, SourceError> {
        let mut headers = HeaderMap::new();
        for value in forwarded {
            headers.append(HEADER, HeaderValue::from_str(value).unwrap());
        }
        source(
            Some(peer.parse().unwrap()),
            &headers,
            &trusted_proxies(trusted)?,
        )
    }

    #[test]
    fn untrusted_peer_cannot_select_or_multiply_source_buckets() {
        for values in [vec![], vec!["192.0.2.1"], vec!["bad"], vec!["a", "b"]] {
            assert_eq!(
                resolve("198.51.100.8", &values, "127.0.0.1")
                    .ok()
                    .as_deref(),
                Some("198.51.100.8")
            );
        }
    }

    #[test]
    fn trusted_proxy_uses_actual_clients_not_one_shared_proxy_bucket() {
        assert_eq!(
            resolve("127.0.0.1", &["192.0.2.1"], "127.0.0.1")
                .ok()
                .as_deref(),
            Some("192.0.2.1")
        );
        assert_eq!(
            resolve("127.0.0.1", &["192.0.2.2"], "127.0.0.1")
                .ok()
                .as_deref(),
            Some("192.0.2.2")
        );
    }

    #[test]
    fn attacker_prefix_is_ignored_while_trusted_suffix_is_peeled() {
        assert_eq!(
            resolve(
                "127.0.0.1",
                &["203.0.113.99, 192.0.2.1, 10.0.0.1"],
                "127.0.0.1,10.0.0.1"
            )
            .ok()
            .as_deref(),
            Some("192.0.2.1")
        );
    }

    #[test]
    fn malformed_missing_duplicate_and_oversized_trusted_headers_are_rejected() {
        for values in [
            vec![],
            vec![""],
            vec!["bad"],
            vec!["192.0.2.1:99"],
            vec!["192.0.2.1,,10.0.0.1"],
            vec!["a", "b"],
        ] {
            assert!(resolve("127.0.0.1", &values, "127.0.0.1").is_err());
        }
        let too_many = vec!["192.0.2.1"; 17].join(",");
        assert!(resolve("127.0.0.1", &[&too_many], "127.0.0.1").is_err());
        assert!(resolve("127.0.0.1", &[&" ".repeat(1025)], "127.0.0.1").is_err());
    }

    #[test]
    fn ipv6_privacy_addresses_share_a_64_bucket_but_other_prefixes_do_not() {
        let first = resolve("2001:db8:1:2::1", &[], "").ok();
        assert_eq!(first, resolve("2001:db8:1:2:ffff::abcd", &[], "").ok());
        assert_ne!(first, resolve("2001:db8:1:3::1", &[], "").ok());
    }

    #[test]
    fn ipv4_mapped_ipv6_does_not_change_trust_or_source_identity() {
        assert_eq!(
            resolve("::ffff:127.0.0.1", &["::ffff:192.0.2.1"], "127.0.0.1")
                .ok()
                .as_deref(),
            Some("192.0.2.1")
        );
    }

    #[test]
    fn exact_proxy_ips_only_and_invalid_configuration_fails_closed() {
        for config in ["*", "0.0.0.0/0", "10.0.0.0/8", "127.0.0.1,", "hostname"] {
            assert!(trusted_proxies(config).is_err());
        }
        assert!(trusted_proxies(&vec!["127.0.0.1"; 33].join(",")).is_err());
        assert_eq!(
            source(None, &HeaderMap::new(), &HashSet::new())
                .ok()
                .as_deref(),
            Some("unknown")
        );
    }
}
