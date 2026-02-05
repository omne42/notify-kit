use std::collections::HashMap;
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
const DEFAULT_SYNC_DNS_POSITIVE_CACHE_TTL: Duration = DEFAULT_PINNED_CLIENT_TTL;
const DEFAULT_SYNC_DNS_NEGATIVE_CACHE_TTL: Duration = Duration::from_secs(5);
const DEFAULT_MAX_SYNC_DNS_CACHE_ENTRIES: usize = 256;
const DEFAULT_MAX_SYNC_DNS_INFLIGHT_HOSTS: usize = 256;

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

#[derive(Clone)]
struct CachedDnsResult {
    result: Result<Arc<Vec<SocketAddr>>, String>,
    expires_at: Instant,
}

struct InflightResolve {
    result: Mutex<Option<Result<Arc<Vec<SocketAddr>>, String>>>,
    cv: Condvar,
}

static PINNED_CLIENT_CACHE: OnceLock<RwLock<HashMap<PinnedClientKey, CachedPinnedClient>>> =
    OnceLock::new();
static DNS_LOOKUP_SEMAPHORE: OnceLock<Arc<Semaphore>> = OnceLock::new();
static SYNC_DNS_LOOKUP_SEMAPHORE: OnceLock<Arc<SyncSemaphore>> = OnceLock::new();
static SYNC_DNS_CACHE: OnceLock<Mutex<HashMap<String, CachedDnsResult>>> = OnceLock::new();
static SYNC_DNS_INFLIGHT: OnceLock<Mutex<HashMap<String, Arc<InflightResolve>>>> = OnceLock::new();

fn dns_lookup_timeout_message() -> String {
    format!("dns lookup timeout (capped at {DEFAULT_DNS_LOOKUP_TIMEOUT:?})")
}

fn pinned_client_cache() -> &'static RwLock<HashMap<PinnedClientKey, CachedPinnedClient>> {
    PINNED_CLIENT_CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

fn dns_lookup_semaphore() -> &'static Arc<Semaphore> {
    DNS_LOOKUP_SEMAPHORE.get_or_init(|| Arc::new(Semaphore::new(DEFAULT_MAX_DNS_LOOKUPS_INFLIGHT)))
}

fn sync_dns_cache() -> &'static Mutex<HashMap<String, CachedDnsResult>> {
    SYNC_DNS_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn sync_dns_inflight() -> &'static Mutex<HashMap<String, Arc<InflightResolve>>> {
    SYNC_DNS_INFLIGHT.get_or_init(|| Mutex::new(HashMap::new()))
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

impl InflightResolve {
    fn new() -> Self {
        Self {
            result: Mutex::new(None),
            cv: Condvar::new(),
        }
    }

    fn set_result(&self, result: Result<Arc<Vec<SocketAddr>>, String>) {
        let mut guard = self.result.lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some(result);
        self.cv.notify_all();
    }

    fn set_result_if_empty(&self, result: Result<Arc<Vec<SocketAddr>>, String>) -> bool {
        let mut guard = self.result.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_some() {
            return false;
        }
        *guard = Some(result);
        self.cv.notify_all();
        true
    }

    fn wait(&self, timeout: Duration) -> Option<Result<Arc<Vec<SocketAddr>>, String>> {
        if timeout == Duration::ZERO {
            return None;
        }

        let deadline = Instant::now() + timeout;
        let mut guard = self.result.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(res) = guard.as_ref() {
                return Some(res.clone());
            }

            let now = Instant::now();
            if now >= deadline {
                return None;
            }

            let remaining = deadline.duration_since(now);
            let (next, wait) = self
                .cv
                .wait_timeout(guard, remaining)
                .unwrap_or_else(|e| e.into_inner());
            guard = next;

            if wait.timed_out() {
                return None;
            }
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

pub(crate) async fn send_reqwest(
    builder: reqwest::RequestBuilder,
    context: &str,
) -> anyhow::Result<reqwest::Response> {
    builder.send().await.map_err(|err| {
        anyhow::anyhow!(
            "{context} request failed ({})",
            sanitize_reqwest_error(&err)
        )
    })
}

pub(crate) fn validate_url_path_prefix(url: &reqwest::Url, prefix: &str) -> anyhow::Result<()> {
    let path = url.path();
    if path.starts_with(prefix) {
        return Ok(());
    }
    Err(anyhow::anyhow!("url path is not allowed"))
}

pub(crate) fn validate_url_resolves_to_public_ip(
    url: &reqwest::Url,
    timeout: Duration,
) -> anyhow::Result<()> {
    resolve_url_to_public_addrs_with_timeout(url, timeout)?;
    Ok(())
}

fn resolve_url_to_public_addrs_with_timeout(
    url: &reqwest::Url,
    timeout: Duration,
) -> anyhow::Result<Vec<SocketAddr>> {
    let Some(host) = url.host_str() else {
        return Err(anyhow::anyhow!("url must have a host"));
    };

    let dns_timeout = timeout.min(DEFAULT_DNS_LOOKUP_TIMEOUT);
    if dns_timeout == Duration::ZERO {
        return Err(anyhow::anyhow!("{}", dns_lookup_timeout_message()));
    }

    let now = Instant::now();
    let host = host.to_ascii_lowercase();

    {
        let mut cache = sync_dns_cache().lock().unwrap_or_else(|e| e.into_inner());
        cache.retain(|_, v| v.expires_at > now);
        if let Some(cached) = cache.get(&host) {
            if cached.expires_at > now {
                return match &cached.result {
                    Ok(addrs) => Ok((**addrs).clone()),
                    Err(msg) => Err(anyhow::anyhow!("{msg}")),
                };
            }
        }
    }

    let deadline = now + dns_timeout;

    let (inflight, leader) = {
        let mut inflight = sync_dns_inflight()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = inflight.get(&host) {
            (existing.clone(), false)
        } else {
            if inflight.len() >= DEFAULT_MAX_SYNC_DNS_INFLIGHT_HOSTS {
                return Err(anyhow::anyhow!("dns lookup failed"));
            }
            let entry = Arc::new(InflightResolve::new());
            inflight.insert(host.clone(), entry.clone());
            (entry, true)
        }
    };

    if leader {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let Some(permit) = sync_dns_lookup_semaphore()
            .clone()
            .acquire_timeout(remaining)
        else {
            let msg = dns_lookup_timeout_message();
            inflight.set_result(Err(msg.clone()));
            {
                let mut cache = sync_dns_cache().lock().unwrap_or_else(|e| e.into_inner());
                cache.retain(|_, v| v.expires_at > now);
                cache.insert(
                    host.clone(),
                    CachedDnsResult {
                        result: Err(msg.clone()),
                        expires_at: now + DEFAULT_SYNC_DNS_NEGATIVE_CACHE_TTL,
                    },
                );
                cap_hashmap_entries(&mut cache, DEFAULT_MAX_SYNC_DNS_CACHE_ENTRIES, &host);
            }
            let mut inflight_map = sync_dns_inflight()
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if inflight_map
                .get(&host)
                .is_some_and(|current| Arc::ptr_eq(current, &inflight))
            {
                inflight_map.remove(&host);
            }
            return Err(anyhow::anyhow!("{msg}"));
        };

        let inflight_entry = inflight.clone();
        let host_key = host.clone();
        let spawn_res = std::thread::Builder::new()
            .name("notify-kit-dns".to_string())
            .spawn(move || {
                let _permit = permit;
                let res = resolve_host_to_public_addrs(&host_key);
                match res {
                    Ok(addrs) => {
                        let addrs = Arc::new(addrs);
                        let now = Instant::now();

                        {
                            let mut cache =
                                sync_dns_cache().lock().unwrap_or_else(|e| e.into_inner());
                            cache.retain(|_, v| v.expires_at > now);
                            cache.insert(
                                host_key.clone(),
                                CachedDnsResult {
                                    result: Ok(addrs.clone()),
                                    expires_at: now + DEFAULT_SYNC_DNS_POSITIVE_CACHE_TTL,
                                },
                            );
                            cap_hashmap_entries(
                                &mut cache,
                                DEFAULT_MAX_SYNC_DNS_CACHE_ENTRIES,
                                &host_key,
                            );
                        }

                        inflight_entry.set_result(Ok(addrs));
                    }
                    Err(err) => {
                        let msg = err.to_string();
                        let now = Instant::now();
                        {
                            let mut cache =
                                sync_dns_cache().lock().unwrap_or_else(|e| e.into_inner());
                            cache.retain(|_, v| v.expires_at > now);
                            cache.insert(
                                host_key.clone(),
                                CachedDnsResult {
                                    result: Err(msg.clone()),
                                    expires_at: now + DEFAULT_SYNC_DNS_NEGATIVE_CACHE_TTL,
                                },
                            );
                            cap_hashmap_entries(
                                &mut cache,
                                DEFAULT_MAX_SYNC_DNS_CACHE_ENTRIES,
                                &host_key,
                            );
                        }
                        inflight_entry.set_result(Err(msg));
                    }
                }

                let mut inflight_map = sync_dns_inflight()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if inflight_map
                    .get(&host_key)
                    .is_some_and(|current| Arc::ptr_eq(current, &inflight_entry))
                {
                    inflight_map.remove(&host_key);
                }
            });
        if spawn_res.is_err() {
            inflight.set_result(Err("dns lookup failed".to_string()));
            let now = Instant::now();
            {
                let mut cache = sync_dns_cache().lock().unwrap_or_else(|e| e.into_inner());
                cache.retain(|_, v| v.expires_at > now);
                cache.insert(
                    host.clone(),
                    CachedDnsResult {
                        result: Err("dns lookup failed".to_string()),
                        expires_at: now + DEFAULT_SYNC_DNS_NEGATIVE_CACHE_TTL,
                    },
                );
                cap_hashmap_entries(&mut cache, DEFAULT_MAX_SYNC_DNS_CACHE_ENTRIES, &host);
            }
            let mut inflight_map = sync_dns_inflight()
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if inflight_map
                .get(&host)
                .is_some_and(|current| Arc::ptr_eq(current, &inflight))
            {
                inflight_map.remove(&host);
            }
            return Err(anyhow::anyhow!("dns lookup failed"));
        }
    }

    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining == Duration::ZERO {
        return Err(anyhow::anyhow!("{}", dns_lookup_timeout_message()));
    }

    match inflight.wait(remaining) {
        Some(Ok(addrs)) => Ok((*addrs).clone()),
        Some(Err(msg)) => Err(anyhow::anyhow!("{msg}")),
        None => {
            let msg = dns_lookup_timeout_message();
            if inflight.set_result_if_empty(Err(msg.clone())) {
                let msg_for_cache = msg.clone();
                let now = Instant::now();
                {
                    let mut cache = sync_dns_cache().lock().unwrap_or_else(|e| e.into_inner());
                    cache.retain(|_, v| v.expires_at > now);
                    cache.insert(
                        host.clone(),
                        CachedDnsResult {
                            result: Err(msg_for_cache),
                            expires_at: now + DEFAULT_SYNC_DNS_NEGATIVE_CACHE_TTL,
                        },
                    );
                    cap_hashmap_entries(&mut cache, DEFAULT_MAX_SYNC_DNS_CACHE_ENTRIES, &host);
                }
                let mut inflight_map = sync_dns_inflight()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if inflight_map
                    .get(&host)
                    .is_some_and(|current| Arc::ptr_eq(current, &inflight))
                {
                    inflight_map.remove(&host);
                }
            }
            Err(anyhow::anyhow!("{msg}"))
        }
    }
}

fn resolve_host_to_public_addrs(host: &str) -> anyhow::Result<Vec<SocketAddr>> {
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
    url: &reqwest::Url,
) -> anyhow::Result<reqwest::Client> {
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
        .map_err(|_| anyhow::anyhow!("dns lookup failed"))??;

    build_http_client_builder(timeout)
        .resolve_to_addrs(&host, &addrs)
        .build()
        .map_err(|err| anyhow::anyhow!("build reqwest client: {err}"))
}

pub(crate) async fn select_http_client(
    base_client: &reqwest::Client,
    timeout: Duration,
    url: &reqwest::Url,
    enforce_public_ip: bool,
) -> anyhow::Result<reqwest::Client> {
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
    if let Some(v4) = addr.to_ipv4() {
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

    // Documentation 2001:db8::/32
    if bytes[0] == 0x20 && bytes[1] == 0x01 && bytes[2] == 0x0d && bytes[3] == 0xb8 {
        return false;
    }

    true
}

pub(crate) async fn read_json_body_limited(
    resp: reqwest::Response,
    max_bytes: usize,
) -> anyhow::Result<serde_json::Value> {
    let buf = read_body_bytes_limited(resp, max_bytes).await?;
    serde_json::from_slice(&buf).map_err(|_| anyhow::anyhow!("decode json failed"))
}

pub(crate) async fn read_text_body_limited(
    resp: reqwest::Response,
    max_bytes: usize,
) -> anyhow::Result<String> {
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
) -> anyhow::Result<Vec<u8>> {
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

    Ok(buf)
}

async fn read_body_bytes_truncated(
    mut resp: reqwest::Response,
    max_bytes: usize,
) -> anyhow::Result<(Vec<u8>, bool)> {
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
    fn ip_global_checks_work_for_common_ranges() {
        assert!(!is_public_ip(IpAddr::from_str("127.0.0.1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("::ffff:127.0.0.1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("10.0.0.1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("::ffff:10.0.0.1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("169.254.1.1").unwrap()));
        assert!(!is_public_ip(IpAddr::from_str("::1").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("8.8.8.8").unwrap()));
        assert!(is_public_ip(IpAddr::from_str("::ffff:8.8.8.8").unwrap()));
    }
}
