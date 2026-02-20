use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use tokio::sync::{Mutex as TokioMutex, RwLock, Semaphore};

pub(crate) const DEFAULT_MAX_RESPONSE_BODY_BYTES: usize = 16 * 1024;
const RESPONSE_BODY_DRAIN_LIMIT_BYTES: usize = 64 * 1024;

const DEFAULT_DNS_LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);
const DEFAULT_MAX_DNS_LOOKUPS_INFLIGHT: usize = 32;
const DEFAULT_PINNED_CLIENT_TTL: Duration = Duration::from_secs(60);
const DEFAULT_MAX_PINNED_CLIENT_CACHE_ENTRIES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PinnedClientKey {
    host: String,
    timeout: Duration,
}

#[derive(Clone)]
struct CachedPinnedClient {
    client: reqwest::Client,
    expires_at: Instant,
}

static PINNED_CLIENT_CACHE: OnceLock<RwLock<HashMap<PinnedClientKey, CachedPinnedClient>>> =
    OnceLock::new();
static PINNED_CLIENT_BUILD_LOCKS: OnceLock<Mutex<HashMap<PinnedClientKey, Weak<TokioMutex<()>>>>> =
    OnceLock::new();
static DNS_LOOKUP_SEMAPHORE: OnceLock<Arc<Semaphore>> = OnceLock::new();
static DNS_LOOKUP_TIMEOUT_MESSAGE: OnceLock<String> = OnceLock::new();

fn dns_lookup_timeout_message() -> &'static str {
    DNS_LOOKUP_TIMEOUT_MESSAGE
        .get_or_init(|| format!("dns lookup timeout (capped at {DEFAULT_DNS_LOOKUP_TIMEOUT:?})"))
        .as_str()
}

fn pinned_client_cache() -> &'static RwLock<HashMap<PinnedClientKey, CachedPinnedClient>> {
    PINNED_CLIENT_CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

fn pinned_client_build_locks() -> &'static Mutex<HashMap<PinnedClientKey, Weak<TokioMutex<()>>>> {
    PINNED_CLIENT_BUILD_LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_pinned_client_build_locks()
-> std::sync::MutexGuard<'static, HashMap<PinnedClientKey, Weak<TokioMutex<()>>>> {
    pinned_client_build_locks()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn cleanup_pinned_client_build_lock_entry(key: &PinnedClientKey) {
    let mut locks = lock_pinned_client_build_locks();
    if locks.get(key).is_some_and(|weak| weak.strong_count() == 0) {
        locks.remove(key);
    }
}

struct PinnedClientBuildLockCleanupGuard {
    key: PinnedClientKey,
    armed: bool,
}

impl PinnedClientBuildLockCleanupGuard {
    fn new(key: PinnedClientKey) -> Self {
        Self { key, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for PinnedClientBuildLockCleanupGuard {
    fn drop(&mut self) {
        if self.armed {
            cleanup_pinned_client_build_lock_entry(&self.key);
        }
    }
}

fn dns_lookup_semaphore() -> &'static Arc<Semaphore> {
    DNS_LOOKUP_SEMAPHORE.get_or_init(|| Arc::new(Semaphore::new(DEFAULT_MAX_DNS_LOOKUPS_INFLIGHT)))
}

fn remaining_dns_timeout(deadline: Instant) -> crate::Result<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining == Duration::ZERO {
        return Err(anyhow::anyhow!(dns_lookup_timeout_message()).into());
    }
    Ok(remaining)
}

fn cap_pinned_client_cache_entries(
    cache: &mut HashMap<PinnedClientKey, CachedPinnedClient>,
    max: usize,
    keep: &PinnedClientKey,
) {
    if max == 0 {
        cache.clear();
        return;
    }

    while cache.len() > max {
        let Some(key) = cache
            .iter()
            .filter(|(key, _)| *key != keep)
            .min_by(|(lhs_key, lhs_val), (rhs_key, rhs_val)| {
                (lhs_val.expires_at, lhs_key.host.as_str(), lhs_key.timeout).cmp(&(
                    rhs_val.expires_at,
                    rhs_key.host.as_str(),
                    rhs_key.timeout,
                ))
            })
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        cache.remove(&key);
    }
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

fn validate_public_addrs<I>(addrs: I) -> crate::Result<Vec<SocketAddr>>
where
    I: IntoIterator<Item = SocketAddr>,
{
    let addrs = addrs.into_iter();
    let (lower, upper) = addrs.size_hint();
    let cap = upper.unwrap_or(lower);
    let mut out: Vec<SocketAddr> = Vec::with_capacity(cap);
    let mut uniq: HashSet<SocketAddr> = HashSet::with_capacity(cap);
    let mut seen_any = false;
    for addr in addrs {
        seen_any = true;
        if !is_public_ip(addr.ip()) {
            return Err(anyhow::anyhow!("resolved ip is not allowed").into());
        }
        if uniq.insert(addr) {
            out.push(addr);
        }
    }

    if !seen_any {
        return Err(anyhow::anyhow!("dns lookup failed").into());
    }

    Ok(out)
}

async fn resolve_url_to_public_addrs_async(
    url: &reqwest::Url,
    timeout: Duration,
) -> crate::Result<Vec<SocketAddr>> {
    let Some(host) = url.host_str() else {
        return Err(anyhow::anyhow!("url must have a host").into());
    };

    let dns_timeout = timeout.min(DEFAULT_DNS_LOOKUP_TIMEOUT);
    if dns_timeout == Duration::ZERO {
        return Err(anyhow::anyhow!(dns_lookup_timeout_message()).into());
    }

    let deadline = Instant::now() + dns_timeout;
    let lookup = {
        let _permit = tokio::time::timeout(
            remaining_dns_timeout(deadline)?,
            dns_lookup_semaphore().acquire(),
        )
        .await
        .map_err(|_| anyhow::anyhow!(dns_lookup_timeout_message()))?
        .map_err(|_| anyhow::anyhow!("dns lookup failed"))?;

        tokio::time::timeout(
            remaining_dns_timeout(deadline)?,
            tokio::net::lookup_host((host, 443)),
        )
        .await
        .map_err(|_| anyhow::anyhow!(dns_lookup_timeout_message()))?
        .map_err(|err| anyhow::anyhow!("dns lookup failed: {err}"))?
    };

    validate_public_addrs(lookup)
}

pub(crate) async fn build_http_client_pinned_async(
    timeout: Duration,
    url: &reqwest::Url,
) -> crate::Result<reqwest::Client> {
    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("url must have a host"))?;

    let addrs = resolve_url_to_public_addrs_async(url, timeout).await?;

    build_http_client_builder(timeout)
        .resolve_to_addrs(host, &addrs)
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
    let key = PinnedClientKey {
        host: host.to_string(),
        timeout,
    };

    let lookup_now = Instant::now();
    {
        let cache = pinned_client_cache().read().await;
        if let Some(cached) = cache.get(&key) {
            if cached.expires_at > lookup_now {
                return Ok(cached.client.clone());
            }
        }
    }

    let mut build_lock_cleanup = PinnedClientBuildLockCleanupGuard::new(key.clone());
    let key_lock = {
        let mut locks = lock_pinned_client_build_locks();
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(existing) = locks.get(&key).and_then(Weak::upgrade) {
            existing
        } else {
            let new_lock = Arc::new(TokioMutex::new(()));
            locks.insert(key.clone(), Arc::downgrade(&new_lock));
            new_lock
        }
    };

    let result: crate::Result<reqwest::Client> = async {
        let _build_guard = key_lock.lock().await;
        let now = Instant::now();
        let cached_client = {
            let cache = pinned_client_cache().read().await;
            cache.get(&key).and_then(|cached| {
                if cached.expires_at > now {
                    Some(cached.client.clone())
                } else {
                    None
                }
            })
        };
        if let Some(client) = cached_client {
            Ok(client)
        } else {
            let client = build_http_client_pinned_async(timeout, url).await?;
            let now = Instant::now();
            {
                let mut cache = pinned_client_cache().write().await;
                cache.retain(|_, v| v.expires_at > now);
                cache.insert(
                    key.clone(),
                    CachedPinnedClient {
                        client: client.clone(),
                        expires_at: now + DEFAULT_PINNED_CLIENT_TTL,
                    },
                );
                cap_pinned_client_cache_entries(
                    &mut cache,
                    DEFAULT_MAX_PINNED_CLIENT_CACHE_ENTRIES,
                    &key,
                );
            }
            Ok(client)
        }
    }
    .await;

    drop(key_lock);
    cleanup_pinned_client_build_lock_entry(&key);
    build_lock_cleanup.disarm();

    result
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
    if let Some(v4) = ipv4_from_ipv6_mapped(addr) {
        return is_public_ipv4(v4);
    }

    if let Some(v4) = ipv4_from_nat64_well_known_prefix(addr) {
        return is_public_ipv4(v4);
    }

    if let Some(v4) = ipv4_from_6to4(addr) {
        return is_public_ipv4(v4);
    }

    let bytes = addr.octets();

    // IPv4-compatible IPv6 (::/96) is deprecated and should never be treated
    // as publicly routable for SSRF checks.
    if bytes[..12] == [0; 12] {
        return false;
    }

    // Unspecified :: / loopback ::1
    if addr.is_unspecified() || addr.is_loopback() {
        return false;
    }

    // Discard-only prefix 100::/64 (RFC6666)
    if bytes[..8] == [0x01, 0x00, 0, 0, 0, 0, 0, 0] {
        return false;
    }

    // Benchmarking 2001:2::/48 (RFC5180)
    if bytes[..6] == [0x20, 0x01, 0x00, 0x02, 0x00, 0x00] {
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

fn ipv4_from_ipv6_mapped(addr: Ipv6Addr) -> Option<Ipv4Addr> {
    let bytes = addr.octets();
    // IPv4-mapped IPv6 (::ffff:0:0/96)
    if bytes[..10] == [0; 10] && bytes[10] == 0xff && bytes[11] == 0xff {
        return Some(Ipv4Addr::new(bytes[12], bytes[13], bytes[14], bytes[15]));
    }
    None
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
    Ok(decode_text_body_lossy(buf, truncated))
}

fn decode_text_body_lossy(buf: Vec<u8>, truncated: bool) -> String {
    let mut out = match String::from_utf8(buf) {
        Ok(text) => text,
        Err(err) => String::from_utf8_lossy(&err.into_bytes()).into_owned(),
    };
    if truncated {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("[truncated]");
    }
    out
}

async fn read_body_bytes_limited(
    mut resp: reqwest::Response,
    max_bytes: usize,
) -> crate::Result<Vec<u8>> {
    if max_bytes == 0 {
        drain_response_body_limited(&mut resp, RESPONSE_BODY_DRAIN_LIMIT_BYTES).await;
        return Err(anyhow::anyhow!("response body too large (response body omitted)").into());
    }

    let mut cap_hint = 0usize;
    if let Some(len) = resp.content_length() {
        if len > max_bytes as u64 {
            drain_response_body_limited(&mut resp, RESPONSE_BODY_DRAIN_LIMIT_BYTES).await;
            return Err(anyhow::anyhow!("response body too large (response body omitted)").into());
        }
        cap_hint = content_length_capacity_hint(len, max_bytes);
    }

    let mut buf = Vec::with_capacity(cap_hint);
    while let Some(chunk) = resp.chunk().await.map_err(|err| {
        anyhow::anyhow!(
            "read response body failed ({})",
            sanitize_reqwest_error(&err)
        )
    })? {
        if chunk.len() > max_bytes.saturating_sub(buf.len()) {
            drain_response_body_limited(&mut resp, RESPONSE_BODY_DRAIN_LIMIT_BYTES).await;
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
        drain_response_body_limited(&mut resp, RESPONSE_BODY_DRAIN_LIMIT_BYTES).await;
        return Ok((Vec::new(), true));
    }

    let mut truncated = false;
    let mut cap_hint = 0usize;
    if let Some(len) = resp.content_length() {
        if len > max_bytes as u64 {
            truncated = true;
        }
        cap_hint = content_length_capacity_hint(len, max_bytes);
    }

    let mut buf = Vec::with_capacity(cap_hint);
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

    if truncated {
        drain_response_body_limited(&mut resp, RESPONSE_BODY_DRAIN_LIMIT_BYTES).await;
    }

    Ok((buf, truncated))
}

async fn drain_response_body_limited(resp: &mut reqwest::Response, mut remaining: usize) {
    while remaining > 0 {
        let Ok(Some(chunk)) = resp.chunk().await else {
            break;
        };
        remaining = remaining.saturating_sub(chunk.len());
    }
}

fn content_length_capacity_hint(content_length: u64, max_bytes: usize) -> usize {
    usize::try_from(content_length)
        .ok()
        .map_or(max_bytes, |len| len.min(max_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;
    use std::str::FromStr;
    use std::time::{Duration, Instant};

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
        assert!(!is_public_ip(IpAddr::from_str("::7f00:1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("64:ff9b::7f00:1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("2002:7f00:1::1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("10.0.0.1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("::ffff:10.0.0.1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("::a00:1").unwrap()));
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
        assert!(!is_public_ip(IpAddr::from_str("100::1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("2001:2::1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("169.254.1.1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("::1").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("8.8.8.8").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("::ffff:8.8.8.8").unwrap()));
        assert!(is_public_ip(
            IpAddr::from_str("2001:4860:4860::8888").unwrap()
        ));
        assert!(!is_public_ip(IpAddr::from_str("::808:808").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("64:ff9b::808:808").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("2002:808:808::1").unwrap()));
    }

    #[test]
    fn remaining_dns_timeout_accepts_future_deadline() {
        let remaining =
            remaining_dns_timeout(Instant::now() + Duration::from_millis(10)).expect("timeout");
        assert!(remaining > Duration::ZERO);
        assert!(remaining <= Duration::from_millis(10));
    }

    #[test]
    fn remaining_dns_timeout_rejects_elapsed_deadline() {
        let err =
            remaining_dns_timeout(Instant::now()).expect_err("elapsed deadline should be rejected");
        assert!(err.to_string().contains("dns lookup timeout"), "{err:#}");
    }

    #[test]
    fn pinned_client_key_keeps_sub_millisecond_timeout_precision() {
        let host = "example.com".to_string();
        let lhs = PinnedClientKey {
            host: host.clone(),
            timeout: Duration::from_micros(500),
        };
        let rhs = PinnedClientKey {
            host,
            timeout: Duration::from_micros(900),
        };
        assert_ne!(lhs, rhs);
    }

    #[test]
    fn decode_text_body_lossy_reuses_valid_utf8_buffer() {
        let bytes = b"ok".to_vec();
        let ptr = bytes.as_ptr();
        let out = decode_text_body_lossy(bytes, false);
        assert_eq!(out, "ok");
        assert_eq!(out.as_ptr(), ptr);
    }

    #[test]
    fn decode_text_body_lossy_handles_invalid_utf8() {
        let out = decode_text_body_lossy(vec![0xff, b'a'], false);
        assert_eq!(out, "\u{fffd}a");
    }

    #[test]
    fn decode_text_body_lossy_marks_truncated_output() {
        let out = decode_text_body_lossy(b"line".to_vec(), true);
        assert_eq!(out, "line\n[truncated]");
    }

    #[test]
    fn select_http_client_cleans_build_lock_on_error() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async {
            let url =
                reqwest::Url::parse("https://lock-cleanup.invalid/webhook").expect("parse url");
            let key = PinnedClientKey {
                host: "lock-cleanup.invalid".to_string(),
                timeout: Duration::ZERO,
            };

            {
                let mut cache = pinned_client_cache().write().await;
                cache.remove(&key);
            }
            {
                let mut locks = lock_pinned_client_build_locks();
                locks.remove(&key);
            }

            let client = build_http_client(Duration::from_millis(10)).expect("build client");
            let err = select_http_client(&client, Duration::ZERO, &url, true)
                .await
                .expect_err("expected dns timeout error");
            assert!(err.to_string().contains("dns lookup timeout"), "{err:#}");

            let locks = lock_pinned_client_build_locks();
            assert!(
                !locks.contains_key(&key),
                "build lock entry should be removed after failed request"
            );
        });
    }

    #[test]
    fn select_http_client_cleans_build_lock_on_cancel() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");

        rt.block_on(async {
            let timeout = Duration::from_secs(1);
            let url =
                reqwest::Url::parse("https://lock-cancel.invalid/webhook").expect("parse url");
            let key = PinnedClientKey {
                host: "lock-cancel.invalid".to_string(),
                timeout,
            };

            {
                let mut cache = pinned_client_cache().write().await;
                cache.remove(&key);
            }
            {
                let mut locks = lock_pinned_client_build_locks();
                locks.remove(&key);
            }

            let semaphore_permits = dns_lookup_semaphore()
                .clone()
                .acquire_many_owned(DEFAULT_MAX_DNS_LOOKUPS_INFLIGHT as u32)
                .await
                .expect("acquire dns semaphore permits");

            let client = build_http_client(timeout).expect("build client");
            let task = tokio::spawn({
                let client = client.clone();
                let url = url.clone();
                async move {
                    let _ = select_http_client(&client, timeout, &url, true).await;
                }
            });

            let mut inserted = false;
            for _ in 0..100 {
                if lock_pinned_client_build_locks().contains_key(&key) {
                    inserted = true;
                    break;
                }
                tokio::task::yield_now().await;
            }
            assert!(inserted, "expected build lock entry before cancellation");

            task.abort();
            let _ = task.await;
            drop(semaphore_permits);
            tokio::task::yield_now().await;

            let locks = lock_pinned_client_build_locks();
            assert!(
                !locks.contains_key(&key),
                "build lock entry should be removed after cancelled request"
            );
        });
    }
}
