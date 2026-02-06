use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::{RwLock, Semaphore};

pub(crate) const DEFAULT_MAX_RESPONSE_BODY_BYTES: usize = 16 * 1024;

const DEFAULT_DNS_LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);
const DEFAULT_MAX_DNS_LOOKUPS_INFLIGHT: usize = 32;
const DEFAULT_PINNED_CLIENT_TTL: Duration = Duration::from_secs(60);
const DEFAULT_MAX_PINNED_CLIENT_CACHE_ENTRIES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PinnedClientKey {
    host: String,
    timeout_ms: u64,
}

#[derive(Clone)]
struct CachedPinnedClient {
    client: reqwest::Client,
    expires_at: Instant,
}

static PINNED_CLIENT_CACHE: OnceLock<RwLock<HashMap<PinnedClientKey, CachedPinnedClient>>> =
    OnceLock::new();
static DNS_LOOKUP_SEMAPHORE: OnceLock<Arc<Semaphore>> = OnceLock::new();
static SYNC_DNS_LOOKUP_SEMAPHORE: OnceLock<Arc<SyncSemaphore>> = OnceLock::new();

fn dns_lookup_timeout_message() -> String {
    format!("dns lookup timeout (capped at {DEFAULT_DNS_LOOKUP_TIMEOUT:?})")
}

fn pinned_client_cache() -> &'static RwLock<HashMap<PinnedClientKey, CachedPinnedClient>> {
    PINNED_CLIENT_CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

fn dns_lookup_semaphore() -> &'static Arc<Semaphore> {
    DNS_LOOKUP_SEMAPHORE.get_or_init(|| Arc::new(Semaphore::new(DEFAULT_MAX_DNS_LOOKUPS_INFLIGHT)))
}

fn cap_hashmap_entries<K: Clone + Eq + Hash, V>(cache: &mut HashMap<K, V>, max: usize, keep: &K) {
    if max == 0 {
        cache.clear();
        return;
    }
    if cache.len() <= max {
        return;
    }

    let to_remove = cache.len() - max;
    let keys: Vec<K> = cache
        .keys()
        .filter(|k| *k != keep)
        .take(to_remove)
        .cloned()
        .collect();
    for k in keys {
        cache.remove(&k);
        if cache.len() <= max {
            break;
        }
    }
}

struct SyncSemaphore {
    max: usize,
    available: Mutex<usize>,
    cv: Condvar,
}

struct SyncPermit {
    sem: Arc<SyncSemaphore>,
}

impl Drop for SyncPermit {
    fn drop(&mut self) {
        self.sem.release();
    }
}

impl SyncSemaphore {
    fn new(max: usize) -> Self {
        Self {
            max,
            available: Mutex::new(max),
            cv: Condvar::new(),
        }
    }

    fn release(&self) {
        let mut available = self.available.lock().unwrap_or_else(|e| e.into_inner());
        *available = (*available + 1).min(self.max);
        self.cv.notify_one();
    }

    fn acquire_timeout(self: &Arc<Self>, timeout: Duration) -> Option<SyncPermit> {
        if timeout == Duration::ZERO {
            return None;
        }
        let deadline = Instant::now() + timeout;

        let mut available = self.available.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if *available > 0 {
                *available -= 1;
                return Some(SyncPermit { sem: self.clone() });
            }

            let now = Instant::now();
            if now >= deadline {
                return None;
            }

            let remaining = deadline.duration_since(now);
            let (guard, wait) = self
                .cv
                .wait_timeout(available, remaining)
                .unwrap_or_else(|e| e.into_inner());
            available = guard;
            if wait.timed_out() {
                return None;
            }
        }
    }
}

fn sync_dns_lookup_semaphore() -> &'static Arc<SyncSemaphore> {
    SYNC_DNS_LOOKUP_SEMAPHORE
        .get_or_init(|| Arc::new(SyncSemaphore::new(DEFAULT_MAX_DNS_LOOKUPS_INFLIGHT)))
}

fn build_http_client_builder(timeout: Duration) -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
}

pub(crate) fn build_http_client(timeout: Duration) -> crate::Result<reqwest::Client> {
    build_http_client_builder(timeout)
        .build()
        .map_err(|err| anyhow::anyhow!("build reqwest client: {err}").into())
}

pub(crate) fn parse_and_validate_https_url_basic(url_str: &str) -> crate::Result<reqwest::Url> {
    let url = reqwest::Url::parse(url_str).map_err(|err| anyhow::anyhow!("invalid url: {err}"))?;

    if url.scheme() != "https" {
        return Err(anyhow::anyhow!("url must use https").into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(anyhow::anyhow!("url must not contain credentials").into());
    }

    let Some(host) = url.host_str() else {
        return Err(anyhow::anyhow!("url must have a host").into());
    };
    if host.eq_ignore_ascii_case("localhost") || host.parse::<std::net::IpAddr>().is_ok() {
        return Err(anyhow::anyhow!("url host is not allowed").into());
    }

    if let Some(port) = url.port() {
        if port != 443 {
            return Err(anyhow::anyhow!("url port is not allowed").into());
        }
    }

    Ok(url)
}

pub(crate) fn parse_and_validate_https_url(
    url_str: &str,
    allowed_hosts: &[&str],
) -> crate::Result<reqwest::Url> {
    let url = parse_and_validate_https_url_basic(url_str)?;
    let Some(host) = url.host_str() else {
        return Err(anyhow::anyhow!("url must have a host").into());
    };

    if !allowed_hosts
        .iter()
        .any(|allowed| host.eq_ignore_ascii_case(allowed))
    {
        return Err(anyhow::anyhow!("url host is not allowed").into());
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

pub(crate) async fn send_reqwest(
    builder: reqwest::RequestBuilder,
    context: &str,
) -> crate::Result<reqwest::Response> {
    builder.send().await.map_err(|err| {
        anyhow::anyhow!(
            "{context} request failed ({})",
            sanitize_reqwest_error(&err)
        )
        .into()
    })
}

pub(crate) fn validate_url_path_prefix(url: &reqwest::Url, prefix: &str) -> crate::Result<()> {
    let path = url.path();
    if prefix.is_empty() {
        return Err(anyhow::anyhow!("url path is not allowed").into());
    }

    if prefix.ends_with('/') {
        if path.starts_with(prefix) {
            return Ok(());
        }
        return Err(anyhow::anyhow!("url path is not allowed").into());
    }

    if path == prefix {
        return Ok(());
    }

    let Some(next) = path.as_bytes().get(prefix.len()) else {
        return Err(anyhow::anyhow!("url path is not allowed").into());
    };

    if path.starts_with(prefix) && *next == b'/' {
        return Ok(());
    }

    Err(anyhow::anyhow!("url path is not allowed").into())
}

pub(crate) fn validate_url_resolves_to_public_ip(
    url: &reqwest::Url,
    timeout: Duration,
) -> crate::Result<()> {
    resolve_url_to_public_addrs_with_timeout(url, timeout)?;
    Ok(())
}

fn resolve_url_to_public_addrs_with_timeout(
    url: &reqwest::Url,
    timeout: Duration,
) -> crate::Result<Vec<SocketAddr>> {
    let Some(host) = url.host_str() else {
        return Err(anyhow::anyhow!("url must have a host").into());
    };

    let dns_timeout = timeout.min(DEFAULT_DNS_LOOKUP_TIMEOUT);
    if dns_timeout == Duration::ZERO {
        return Err(anyhow::anyhow!("{}", dns_lookup_timeout_message()).into());
    }

    let deadline = Instant::now() + dns_timeout;

    let remaining = deadline.saturating_duration_since(Instant::now());
    let Some(permit) = sync_dns_lookup_semaphore()
        .clone()
        .acquire_timeout(remaining)
    else {
        return Err(anyhow::anyhow!("{}", dns_lookup_timeout_message()).into());
    };

    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let host = host.to_ascii_lowercase();
    std::thread::Builder::new()
        .name("notify-kit-dns".to_string())
        .spawn(move || {
            let _permit = permit;
            let res = resolve_host_to_public_addrs(&host);
            let _ = tx.send(res);
        })
        .map_err(|err| anyhow::anyhow!("dns lookup spawn failed: {err}"))?;

    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining == Duration::ZERO {
        return Err(anyhow::anyhow!("{}", dns_lookup_timeout_message()).into());
    }

    match rx.recv_timeout(remaining) {
        Ok(res) => res,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            Err(anyhow::anyhow!("{}", dns_lookup_timeout_message()).into())
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            Err(anyhow::anyhow!("dns lookup failed").into())
        }
    }
}

fn resolve_host_to_public_addrs(host: &str) -> crate::Result<Vec<SocketAddr>> {
    let addrs = (host, 443)
        .to_socket_addrs()
        .map_err(|err| anyhow::anyhow!("dns lookup failed: {err}"))?;

    let mut out: Vec<SocketAddr> = Vec::new();
    let mut uniq: HashSet<SocketAddr> = HashSet::new();
    let mut seen = 0usize;
    for addr in addrs {
        seen += 1;
        if !is_public_ip(addr.ip()) {
            return Err(anyhow::anyhow!("resolved ip is not allowed").into());
        }
        if uniq.insert(addr) {
            out.push(addr);
        }
    }

    if seen == 0 {
        return Err(anyhow::anyhow!("dns lookup failed").into());
    }

    Ok(out)
}

pub(crate) async fn build_http_client_pinned_async(
    timeout: Duration,
    url: &reqwest::Url,
) -> crate::Result<reqwest::Client> {
    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("url must have a host"))?
        .to_string();

    let dns_timeout = timeout.min(DEFAULT_DNS_LOOKUP_TIMEOUT);
    let permit = tokio::time::timeout(dns_timeout, dns_lookup_semaphore().clone().acquire_owned())
        .await
        .map_err(|_| anyhow::anyhow!("{}", dns_lookup_timeout_message()))?
        .map_err(|_| anyhow::anyhow!("dns lookup failed"))?;
    let lookup = tokio::task::spawn_blocking({
        let host = host.clone();
        move || {
            let _permit = permit;
            resolve_host_to_public_addrs(&host)
        }
    });
    let addrs = tokio::time::timeout(dns_timeout, lookup)
        .await
        .map_err(|_| anyhow::anyhow!("{}", dns_lookup_timeout_message()))?
        .map_err(|err| anyhow::anyhow!("dns lookup failed: {err}"))??;

    build_http_client_builder(timeout)
        .resolve_to_addrs(&host, &addrs)
        .build()
        .map_err(|err| anyhow::anyhow!("build reqwest client: {err}").into())
}

pub(crate) async fn select_http_client(
    base_client: &reqwest::Client,
    timeout: Duration,
    url: &reqwest::Url,
    enforce_public_ip: bool,
) -> crate::Result<reqwest::Client> {
    if !enforce_public_ip {
        return Ok(base_client.clone());
    }

    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("url must have a host"))?;
    let timeout_ms = timeout.as_millis().min(u64::MAX as u128) as u64;
    let key = PinnedClientKey {
        host: host.to_string(),
        timeout_ms,
    };
    let key_for_eviction = key.clone();

    let now = Instant::now();
    {
        let cache = pinned_client_cache().read().await;
        if let Some(cached) = cache.get(&key) {
            if cached.expires_at > now {
                return Ok(cached.client.clone());
            }
        }
    }

    let client = build_http_client_pinned_async(timeout, url).await?;

    {
        let mut cache = pinned_client_cache().write().await;
        cache.retain(|_, v| v.expires_at > now);
        cache.insert(
            key,
            CachedPinnedClient {
                client: client.clone(),
                expires_at: now + DEFAULT_PINNED_CLIENT_TTL,
            },
        );
        cap_hashmap_entries(
            &mut cache,
            DEFAULT_MAX_PINNED_CLIENT_CACHE_ENTRIES,
            &key_for_eviction,
        );
    }

    Ok(client)
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

    // IETF protocol assignments (RFC6890)
    if (a, b, c) == (192, 0, 0) {
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

    // 6to4 relay anycast (RFC3068; deprecated)
    if (a, b, c) == (192, 88, 99) {
        return false;
    }

    // AS112 (RFC7534)
    if (a, b, c) == (192, 31, 196) {
        return false;
    }

    // AMT (RFC7450)
    if (a, b, c) == (192, 52, 193) {
        return false;
    }

    // Direct Delegation AS112 (RFC7535)
    if (a, b, c) == (192, 175, 48) {
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
    if let Some(v4) = addr.to_ipv4() {
        return is_public_ipv4(v4);
    }

    if let Some(v4) = ipv4_from_nat64_well_known_prefix(addr) {
        return is_public_ipv4(v4);
    }

    if let Some(v4) = ipv4_from_6to4(addr) {
        return is_public_ipv4(v4);
    }

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

    // Site-local fec0::/10 (deprecated; treat as non-public)
    if bytes[0] == 0xfe && (bytes[1] & 0xc0) == 0xc0 {
        return false;
    }

    // Documentation 2001:db8::/32
    if bytes[0] == 0x20 && bytes[1] == 0x01 && bytes[2] == 0x0d && bytes[3] == 0xb8 {
        return false;
    }

    true
}

fn ipv4_from_nat64_well_known_prefix(addr: Ipv6Addr) -> Option<Ipv4Addr> {
    let bytes = addr.octets();
    // NAT64 Well-Known Prefix (RFC6052): 64:ff9b::/96
    if bytes[..12] == [0x00, 0x64, 0xff, 0x9b, 0, 0, 0, 0, 0, 0, 0, 0] {
        return Some(Ipv4Addr::new(bytes[12], bytes[13], bytes[14], bytes[15]));
    }
    None
}

fn ipv4_from_6to4(addr: Ipv6Addr) -> Option<Ipv4Addr> {
    let bytes = addr.octets();
    // 6to4 (RFC3056; deprecated): 2002::/16 embeds an IPv4 address.
    if bytes[0] == 0x20 && bytes[1] == 0x02 {
        return Some(Ipv4Addr::new(bytes[2], bytes[3], bytes[4], bytes[5]));
    }
    None
}

pub(crate) async fn read_json_body_limited(
    resp: reqwest::Response,
    max_bytes: usize,
) -> crate::Result<serde_json::Value> {
    let buf = read_body_bytes_limited(resp, max_bytes).await?;
    serde_json::from_slice(&buf).map_err(|err| anyhow::anyhow!("decode json failed: {err}").into())
}

pub(crate) async fn read_text_body_limited(
    resp: reqwest::Response,
    max_bytes: usize,
) -> crate::Result<String> {
    let (buf, truncated) = read_body_bytes_truncated(resp, max_bytes).await?;
    let mut out = String::from_utf8_lossy(&buf).into_owned();
    if truncated {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("[truncated]");
    }
    Ok(out)
}

async fn read_body_bytes_limited(
    mut resp: reqwest::Response,
    max_bytes: usize,
) -> crate::Result<Vec<u8>> {
    if max_bytes == 0 {
        return Err(anyhow::anyhow!("response body too large (response body omitted)").into());
    }

    if let Some(len) = resp.content_length() {
        if len > max_bytes as u64 {
            return Err(anyhow::anyhow!("response body too large (response body omitted)").into());
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
            return Err(anyhow::anyhow!("response body too large (response body omitted)").into());
        }
        buf.extend_from_slice(&chunk);
    }

    Ok(buf)
}

async fn read_body_bytes_truncated(
    mut resp: reqwest::Response,
    max_bytes: usize,
) -> crate::Result<(Vec<u8>, bool)> {
    if max_bytes == 0 {
        return Ok((Vec::new(), true));
    }

    let mut truncated = false;
    if let Some(len) = resp.content_length() {
        if len > max_bytes as u64 {
            truncated = true;
        }
    }

    let mut buf = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|err| {
        anyhow::anyhow!(
            "read response body failed ({})",
            sanitize_reqwest_error(&err)
        )
    })? {
        if buf.len() >= max_bytes {
            truncated = true;
            break;
        }

        let remaining = max_bytes - buf.len();
        if chunk.len() > remaining {
            buf.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }

        buf.extend_from_slice(&chunk);
    }

    Ok((buf, truncated))
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
    fn path_prefix_is_segment_boundary_matched() {
        let url = reqwest::Url::parse("https://example.com/send").expect("parse url");
        validate_url_path_prefix(&url, "/send").expect("exact match");

        let url = reqwest::Url::parse("https://example.com/send/ok").expect("parse url");
        validate_url_path_prefix(&url, "/send").expect("segment match");

        let url = reqwest::Url::parse("https://example.com/sendMessage").expect("parse url");
        validate_url_path_prefix(&url, "/send").expect_err("should not match prefix substring");

        let url = reqwest::Url::parse("https://example.com/services/x").expect("parse url");
        validate_url_path_prefix(&url, "/services/").expect("trailing slash prefix");

        let url = reqwest::Url::parse("https://example.com/servicesX").expect("parse url");
        validate_url_path_prefix(&url, "/services/").expect_err("trailing slash prevents match");
    }

    #[test]
    fn ip_global_checks_work_for_common_ranges() {
        assert!(!is_public_ip(IpAddr::from_str("127.0.0.1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("::ffff:127.0.0.1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("64:ff9b::7f00:1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("2002:7f00:1::1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("10.0.0.1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("::ffff:10.0.0.1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("64:ff9b::a00:1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("2002:a00:1::1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("192.0.0.1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("64:ff9b::c000:1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("2002:c000:1::1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("192.88.99.1").unwrap()));
        assert!(!is_public_ip(
            IpAddr::from_str("64:ff9b::c058:6301").unwrap()
        ));
        assert!(!is_public_ip(
            IpAddr::from_str("2002:c058:6301::1").unwrap()
        ));
        assert!(!is_public_ip(IpAddr::from_str("192.31.196.1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("192.52.193.1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("192.175.48.1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("fec0::1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("169.254.1.1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("::1").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("8.8.8.8").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("::ffff:8.8.8.8").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("64:ff9b::808:808").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("2002:808:808::1").unwrap()));
    }
}
