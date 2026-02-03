use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::time::Duration;

pub(crate) const DEFAULT_MAX_RESPONSE_BODY_BYTES: usize = 16 * 1024;

fn build_http_client_builder(timeout: Duration) -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
}

pub(crate) fn build_http_client(timeout: Duration) -> anyhow::Result<reqwest::Client> {
    build_http_client_builder(timeout)
        .build()
        .map_err(|err| anyhow::anyhow!("build reqwest client: {err}"))
}

pub(crate) fn parse_and_validate_https_url_basic(url_str: &str) -> anyhow::Result<reqwest::Url> {
    let url = reqwest::Url::parse(url_str).map_err(|err| anyhow::anyhow!("invalid url: {err}"))?;

    if url.scheme() != "https" {
        return Err(anyhow::anyhow!("url must use https"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(anyhow::anyhow!("url must not contain credentials"));
    }

    let Some(host) = url.host_str() else {
        return Err(anyhow::anyhow!("url must have a host"));
    };
    if host.eq_ignore_ascii_case("localhost") || host.parse::<std::net::IpAddr>().is_ok() {
        return Err(anyhow::anyhow!("url host is not allowed"));
    }

    if let Some(port) = url.port() {
        if port != 443 {
            return Err(anyhow::anyhow!("url port is not allowed"));
        }
    }

    Ok(url)
}

pub(crate) fn parse_and_validate_https_url(
    url_str: &str,
    allowed_hosts: &[&str],
) -> anyhow::Result<reqwest::Url> {
    let url = parse_and_validate_https_url_basic(url_str)?;
    let Some(host) = url.host_str() else {
        return Err(anyhow::anyhow!("url must have a host"));
    };

    if !allowed_hosts
        .iter()
        .any(|allowed| host.eq_ignore_ascii_case(allowed))
    {
        return Err(anyhow::anyhow!("url host is not allowed"));
    }

    Ok(url)
}

pub(crate) fn redact_url_str(url_str: &str) -> String {
    let Ok(url) = reqwest::Url::parse(url_str) else {
        return "<redacted>".to_string();
    };
    redact_url(&url)
}

pub(crate) fn redact_url(url: &reqwest::Url) -> String {
    match (url.scheme(), url.host_str()) {
        (scheme, Some(host)) => format!("{scheme}://{host}/<redacted>"),
        _ => "<redacted>".to_string(),
    }
}

pub(crate) fn sanitize_reqwest_error(err: &reqwest::Error) -> &'static str {
    if err.is_timeout() {
        "timeout"
    } else if err.is_connect() {
        "connect"
    } else if err.is_request() {
        "request"
    } else if err.is_decode() {
        "decode"
    } else {
        "unknown"
    }
}

pub(crate) fn validate_url_path_prefix(url: &reqwest::Url, prefix: &str) -> anyhow::Result<()> {
    let path = url.path();
    if path.starts_with(prefix) {
        return Ok(());
    }
    Err(anyhow::anyhow!("url path is not allowed"))
}

pub(crate) fn validate_url_resolves_to_public_ip(url: &reqwest::Url) -> anyhow::Result<()> {
    resolve_url_to_public_addrs(url)?;
    Ok(())
}

fn resolve_url_to_public_addrs(url: &reqwest::Url) -> anyhow::Result<Vec<SocketAddr>> {
    let Some(host) = url.host_str() else {
        return Err(anyhow::anyhow!("url must have a host"));
    };

    let addrs = (host, 443)
        .to_socket_addrs()
        .map_err(|_| anyhow::anyhow!("dns lookup failed"))?;

    let mut out: Vec<SocketAddr> = Vec::new();
    let mut seen = 0usize;
    for addr in addrs {
        seen += 1;
        if !is_public_ip(addr.ip()) {
            return Err(anyhow::anyhow!("resolved ip is not allowed"));
        }
        if !out.contains(&addr) {
            out.push(addr);
        }
    }

    if seen == 0 {
        return Err(anyhow::anyhow!("dns lookup failed"));
    }

    Ok(out)
}

pub(crate) async fn build_http_client_pinned_async(
    timeout: Duration,
    url: reqwest::Url,
) -> anyhow::Result<reqwest::Client> {
    let (host, addrs) = tokio::task::spawn_blocking(move || {
        let host = url
            .host_str()
            .ok_or_else(|| anyhow::anyhow!("url must have a host"))?
            .to_string();
        let addrs = resolve_url_to_public_addrs(&url)?;
        Ok::<_, anyhow::Error>((host, addrs))
    })
    .await
    .map_err(|_| anyhow::anyhow!("dns lookup failed"))??;

    build_http_client_builder(timeout)
        .resolve_to_addrs(&host, &addrs)
        .build()
        .map_err(|err| anyhow::anyhow!("build reqwest client: {err}"))
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(addr) => is_public_ipv4(addr),
        IpAddr::V6(addr) => is_public_ipv6(addr),
    }
}

fn is_public_ipv4(addr: Ipv4Addr) -> bool {
    let [a, b, c, _d] = addr.octets();

    // Unspecified / "this host"
    if a == 0 {
        return false;
    }

    // Private ranges (RFC1918)
    if a == 10 {
        return false;
    }
    if a == 172 && (16..=31).contains(&b) {
        return false;
    }
    if a == 192 && b == 168 {
        return false;
    }

    // Carrier-grade NAT (RFC6598)
    if a == 100 && (64..=127).contains(&b) {
        return false;
    }

    // Loopback
    if a == 127 {
        return false;
    }

    // Link-local
    if a == 169 && b == 254 {
        return false;
    }

    // Documentation ranges (RFC5737)
    if (a, b, c) == (192, 0, 2) || (a, b, c) == (198, 51, 100) || (a, b, c) == (203, 0, 113) {
        return false;
    }

    // Network interconnect device benchmark testing (RFC2544)
    if a == 198 && (b == 18 || b == 19) {
        return false;
    }

    // Multicast (224/4) and reserved (240/4)
    if a >= 224 {
        return false;
    }

    true
}

fn is_public_ipv6(addr: Ipv6Addr) -> bool {
    let bytes = addr.octets();

    // Unspecified :: / loopback ::1
    if addr.is_unspecified() || addr.is_loopback() {
        return false;
    }

    // Multicast ff00::/8
    if bytes[0] == 0xff {
        return false;
    }

    // Unique local fc00::/7
    if (bytes[0] & 0xfe) == 0xfc {
        return false;
    }

    // Link-local fe80::/10
    if bytes[0] == 0xfe && (bytes[1] & 0xc0) == 0x80 {
        return false;
    }

    // Documentation 2001:db8::/32
    if bytes[0] == 0x20 && bytes[1] == 0x01 && bytes[2] == 0x0d && bytes[3] == 0xb8 {
        return false;
    }

    true
}

pub(crate) async fn read_json_body_limited(
    mut resp: reqwest::Response,
    max_bytes: usize,
) -> anyhow::Result<serde_json::Value> {
    if max_bytes == 0 {
        return Err(anyhow::anyhow!(
            "response body too large (response body omitted)"
        ));
    }

    if let Some(len) = resp.content_length() {
        if len > max_bytes as u64 {
            return Err(anyhow::anyhow!(
                "response body too large (response body omitted)"
            ));
        }
    }

    let mut buf = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|err| {
        anyhow::anyhow!(
            "read response body failed ({})",
            sanitize_reqwest_error(&err)
        )
    })? {
        if buf.len() + chunk.len() > max_bytes {
            return Err(anyhow::anyhow!(
                "response body too large (response body omitted)"
            ));
        }
        buf.extend_from_slice(&chunk);
    }

    serde_json::from_slice(&buf).map_err(|_| anyhow::anyhow!("decode json failed"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;
    use std::str::FromStr;

    #[test]
    fn redact_url_str_never_leaks_path_or_query() {
        let url = "https://hooks.slack.com/services/secret?token=top";
        let redacted = redact_url_str(url);
        assert!(!redacted.contains("secret"), "{redacted}");
        assert!(!redacted.contains("token"), "{redacted}");
        assert!(redacted.contains("hooks.slack.com"), "{redacted}");
        assert!(redacted.contains("<redacted>"), "{redacted}");
    }

    #[test]
    fn rejects_credentials() {
        let err = parse_and_validate_https_url(
            "https://u:p@hooks.slack.com/services/x",
            &["hooks.slack.com"],
        )
        .expect_err("expected invalid url");
        assert!(err.to_string().contains("credentials"), "{err:#}");
    }

    #[test]
    fn rejects_non_443_port() {
        let err = parse_and_validate_https_url(
            "https://hooks.slack.com:444/services/x",
            &["hooks.slack.com"],
        )
        .expect_err("expected invalid url");
        assert!(err.to_string().contains("port"), "{err:#}");
    }

    #[test]
    fn ip_global_checks_work_for_common_ranges() {
        assert!(!is_public_ip(IpAddr::from_str("127.0.0.1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("10.0.0.1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("169.254.1.1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("::1").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("8.8.8.8").unwrap()));
    }
}
