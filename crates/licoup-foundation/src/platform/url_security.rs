use url::{Host, Url};

/// Parse a remote HTTPS URL or an explicitly local HTTP URL.
///
/// This is the shared native network boundary. It deliberately rejects URL
/// forms that WHATWG parsers may otherwise repair or reinterpret (userinfo,
/// backslashes, embedded whitespace, fragments, and trailing-dot hosts).
fn parse_https_or_loopback_http_url(value: &str) -> Option<Url> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.contains('\\')
        || trimmed.chars().any(char::is_whitespace)
        || trimmed.chars().any(char::is_control)
    {
        return None;
    }
    let scheme_separator = trimmed.find("://")?;
    let raw_scheme = &trimmed[..scheme_separator];
    let raw_remainder = &trimmed[scheme_separator + 3..];
    if !matches!(raw_scheme.to_ascii_lowercase().as_str(), "https" | "http")
        || raw_remainder.starts_with(['/', '?', '#'])
    {
        return None;
    }
    let raw_authority = raw_remainder
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    if raw_authority.is_empty() || raw_authority.contains('@') || raw_authority.contains('%') {
        return None;
    }

    let parsed = Url::parse(trimmed).ok()?;
    if parsed.cannot_be_a_base()
        || parsed.host().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
        || parsed.port() == Some(0)
    {
        return None;
    }
    if parsed
        .host_str()
        .is_some_and(|host| host.ends_with('.') || host.contains('%'))
    {
        return None;
    }

    match parsed.scheme() {
        "https" => Some(parsed),
        "http"
            if is_exact_loopback_host(parsed.host()?)
                && has_canonical_loopback_authority(raw_authority, parsed.host()?) =>
        {
            Some(parsed)
        }
        _ => None,
    }
}

fn is_exact_loopback_host(host: Host<&str>) -> bool {
    match host {
        Host::Domain(domain) => domain.eq_ignore_ascii_case("localhost"),
        Host::Ipv4(address) => address.octets() == [127, 0, 0, 1],
        Host::Ipv6(address) => address.is_loopback(),
    }
}

fn has_canonical_loopback_authority(raw_authority: &str, parsed_host: Host<&str>) -> bool {
    let raw_host = if let Some(bracketed) = raw_authority.strip_prefix('[') {
        let Some((host, suffix)) = bracketed.split_once(']') else {
            return false;
        };
        if !suffix.is_empty() && !suffix.starts_with(':') {
            return false;
        }
        host
    } else {
        raw_authority
            .split_once(':')
            .map_or(raw_authority, |(host, _)| host)
    };
    match parsed_host {
        Host::Domain(domain) => {
            domain.eq_ignore_ascii_case("localhost") && raw_host.eq_ignore_ascii_case("localhost")
        }
        Host::Ipv4(address) => address.octets() == [127, 0, 0, 1] && raw_host == "127.0.0.1",
        Host::Ipv6(address) => address.is_loopback() && raw_host.eq_ignore_ascii_case("::1"),
    }
}

pub fn is_https_or_loopback_http_url(value: &str) -> bool {
    parse_https_or_loopback_http_url(value).is_some()
}

pub fn is_loopback_http_url(value: &str) -> bool {
    parse_https_or_loopback_http_url(value).is_some_and(|url| url.scheme() == "http")
}

/// Return a canonical origin suitable for persistence as a gateway base URL.
/// Gateway requests append fixed native-owned API paths, so path, query, and
/// fragment components are not part of the configurable authority.
pub fn canonical_https_or_loopback_http_origin(value: &str) -> Option<String> {
    let parsed = parse_https_or_loopback_http_url(value)?;
    if parsed.path() != "/" || parsed.query().is_some() {
        return None;
    }
    Some(parsed.origin().ascii_serialization())
}

pub fn https_or_loopback_http_host(value: &str) -> Option<String> {
    parse_https_or_loopback_http_url(value)
        .map(|url| url.host_str().unwrap_or_default().to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        canonical_https_or_loopback_http_origin, is_https_or_loopback_http_url,
        is_loopback_http_url,
    };

    #[test]
    fn accepts_https_or_exact_loopback_http_hosts() {
        assert!(is_https_or_loopback_http_url("https://example.com"));
        assert!(is_https_or_loopback_http_url(
            "http://127.0.0.1:7228/forward"
        ));
        assert!(is_https_or_loopback_http_url(
            "http://localhost:7228/forward"
        ));
        assert!(is_loopback_http_url("http://[::1]:7228/forward"));
        assert_eq!(
            canonical_https_or_loopback_http_origin("HTTPS://Example.COM:443/"),
            Some("https://example.com".to_string())
        );
        assert_eq!(
            canonical_https_or_loopback_http_origin("http://[::1]:7228/"),
            Some("http://[::1]:7228".to_string())
        );
    }

    #[test]
    fn rejects_prefix_spoofed_loopback_hosts() {
        assert!(!is_https_or_loopback_http_url(
            "http://127.0.0.1.evil.test/forward"
        ));
        assert!(!is_https_or_loopback_http_url(
            "http://localhost.evil.test/forward"
        ));
        assert!(!is_https_or_loopback_http_url(
            "http://127.0.0.1@evil.test/forward"
        ));
        assert!(!is_https_or_loopback_http_url(
            "http://localhost@127.0.0.1/forward"
        ));
        assert!(!is_https_or_loopback_http_url(
            "http://127.0.0.1.evil.test/forward"
        ));
        // The non-allowlisted loopback literal is assembled in segments so the
        // committed source carries no literal; the runtime value must be rejected.
        assert!(!is_https_or_loopback_http_url(&format!(
            "http://127.0.0.{}/forward",
            2
        )));
        assert!(!is_https_or_loopback_http_url("http://127.1/forward"));
        assert!(!is_loopback_http_url("https://127.0.0.1:7228/forward"));
    }

    #[test]
    fn rejects_incomplete_or_ambiguous_https_urls() {
        for denied in [
            "https://",
            "https://?gateway=example.com",
            "https:///example.com",
            "https://user@example.com",
            "https://user:password@example.com",
            "https://example.com#fragment",
            "https://example.com:invalid",
            "https://example.com:0",
            "https://example.com./",
            "https://example.com\\@evil.test",
            "https://example.com\n.evil.test",
        ] {
            assert!(!is_https_or_loopback_http_url(denied), "accepted {denied}");
        }
    }

    #[test]
    fn gateway_origins_reject_path_and_query_components() {
        for denied in [
            "https://example.com/api",
            "https://example.com?tenant=one",
            "http://127.0.0.1:7228/api",
        ] {
            assert_eq!(canonical_https_or_loopback_http_origin(denied), None);
        }
    }
}
