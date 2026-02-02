use std::time::Duration;

use crate::Event;
use crate::sinks::http::{
    DEFAULT_MAX_RESPONSE_BODY_BYTES, build_http_client, parse_and_validate_https_url,
    parse_and_validate_https_url_basic, read_json_body_limited, redact_url, sanitize_reqwest_error,
    validate_url_path_prefix, validate_url_resolves_to_public_ip_async,
};
use crate::sinks::text::{TextLimits, format_event_body_and_tags_limited, truncate_chars};
use crate::sinks::{BoxFuture, Sink};

const SERVERCHAN_TURBO_ALLOWED_HOSTS: [&str; 1] = ["sctapi.ftqq.com"];

#[non_exhaustive]
#[derive(Clone)]
pub struct ServerChanConfig {
    pub send_key: String,
    pub timeout: Duration,
    pub max_chars: usize,
    pub enforce_public_ip: bool,
}

impl std::fmt::Debug for ServerChanConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerChanConfig")
            .field("send_key", &"<redacted>")
            .field("timeout", &self.timeout)
            .field("max_chars", &self.max_chars)
            .field("enforce_public_ip", &self.enforce_public_ip)
            .finish()
    }
}

impl ServerChanConfig {
    pub fn new(send_key: impl Into<String>) -> Self {
        Self {
            send_key: send_key.into(),
            timeout: Duration::from_secs(2),
            max_chars: 16 * 1024,
            enforce_public_ip: true,
        }
    }

    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    #[must_use]
    pub fn with_max_chars(mut self, max_chars: usize) -> Self {
        self.max_chars = max_chars;
        self
    }

    #[must_use]
    pub fn with_public_ip_check(mut self, enforce_public_ip: bool) -> Self {
        self.enforce_public_ip = enforce_public_ip;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServerChanKind {
    Turbo,
    Sc3,
}

pub struct ServerChanSink {
    api_url: reqwest::Url,
    kind: ServerChanKind,
    client: reqwest::Client,
    max_chars: usize,
    enforce_public_ip: bool,
}

impl std::fmt::Debug for ServerChanSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerChanSink")
            .field("api_url", &redact_url(&self.api_url))
            .field("kind", &self.kind)
            .field("max_chars", &self.max_chars)
            .field("enforce_public_ip", &self.enforce_public_ip)
            .finish_non_exhaustive()
    }
}

impl ServerChanSink {
    pub fn new(config: ServerChanConfig) -> anyhow::Result<Self> {
        if config.send_key.trim().is_empty() {
            return Err(anyhow::anyhow!("serverchan send_key must not be empty"));
        }

        let (kind, api_url) = build_serverchan_url(&config.send_key)?;
        let api_url_str = api_url.as_str().to_string();

        let api_url = match kind {
            ServerChanKind::Turbo => {
                let url =
                    parse_and_validate_https_url(&api_url_str, &SERVERCHAN_TURBO_ALLOWED_HOSTS)?;
                validate_url_path_prefix(&url, "/")?;
                url
            }
            ServerChanKind::Sc3 => {
                let url = parse_and_validate_https_url_basic(&api_url_str)?;
                validate_url_path_prefix(&url, "/send/")?;
                url
            }
        };

        let client = build_http_client(config.timeout)?;
        Ok(Self {
            api_url,
            kind,
            client,
            max_chars: config.max_chars,
            enforce_public_ip: config.enforce_public_ip,
        })
    }

    fn build_payload(event: &Event, max_chars: usize) -> serde_json::Value {
        let title = truncate_chars(&event.title, 256);
        let desp = format_event_body_and_tags_limited(event, TextLimits::new(max_chars));
        serde_json::json!({ "title": title, "desp": desp })
    }
}

fn build_serverchan_url(send_key: &str) -> anyhow::Result<(ServerChanKind, reqwest::Url)> {
    let send_key = send_key.trim();

    if let Some(rest) = send_key.strip_prefix("sctp") {
        let Some(pos) = rest.find('t') else {
            return Err(anyhow::anyhow!("invalid serverchan send_key"));
        };
        let (uid_str, _tail) = rest.split_at(pos);
        if uid_str.is_empty() || !uid_str.chars().all(|ch| ch.is_ascii_digit()) {
            return Err(anyhow::anyhow!("invalid serverchan send_key"));
        }
        let uid: u64 = uid_str
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid serverchan send_key"))?;

        let host = format!("{uid}.push.ft07.com");
        let url_str = format!("https://{host}/send/{send_key}.send");
        let url =
            reqwest::Url::parse(&url_str).map_err(|err| anyhow::anyhow!("invalid url: {err}"))?;
        return Ok((ServerChanKind::Sc3, url));
    }

    let url_str = format!("https://sctapi.ftqq.com/{send_key}.send");
    let url = reqwest::Url::parse(&url_str).map_err(|err| anyhow::anyhow!("invalid url: {err}"))?;
    Ok((ServerChanKind::Turbo, url))
}

impl Sink for ServerChanSink {
    fn name(&self) -> &'static str {
        "serverchan"
    }

    fn send<'a>(&'a self, event: &'a Event) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            if self.enforce_public_ip {
                validate_url_resolves_to_public_ip_async(self.api_url.clone()).await?;
            }

            let payload = Self::build_payload(event, self.max_chars);

            let resp = self
                .client
                .post(self.api_url.clone())
                .json(&payload)
                .send()
                .await
                .map_err(|err| {
                    anyhow::anyhow!(
                        "serverchan request failed ({})",
                        sanitize_reqwest_error(&err)
                    )
                })?;

            let status = resp.status();
            if !status.is_success() {
                return Err(anyhow::anyhow!(
                    "serverchan http error: {status} (response body omitted)"
                ));
            }

            let body = read_json_body_limited(resp, DEFAULT_MAX_RESPONSE_BODY_BYTES).await?;
            let code = body["code"]
                .as_i64()
                .or_else(|| body["errno"].as_i64())
                .unwrap_or(0);
            if code == 0 {
                return Ok(());
            }

            Err(anyhow::anyhow!(
                "serverchan api error: code={code} (response body omitted)"
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Severity;
    use crate::sinks::http::redact_url_str;

    #[test]
    fn builds_expected_payload() {
        let event = Event::new("turn_completed", Severity::Success, "done")
            .with_body("ok")
            .with_tag("thread_id", "t1");

        let payload = ServerChanSink::build_payload(&event, 16 * 1024);
        assert_eq!(payload["title"].as_str().unwrap_or(""), "done");
        let desp = payload["desp"].as_str().unwrap_or("");
        assert!(desp.contains("ok"));
        assert!(desp.contains("thread_id=t1"));
    }

    #[test]
    fn build_url_supports_turbo_and_sc3() {
        let (kind, url) = build_serverchan_url("SCT123tABC").expect("turbo url");
        assert_eq!(kind, ServerChanKind::Turbo);
        assert_eq!(url.host_str().unwrap_or(""), "sctapi.ftqq.com");
        assert!(url.path().ends_with(".send"));

        let (kind, url) = build_serverchan_url("sctp123tABC").expect("sc3 url");
        assert_eq!(kind, ServerChanKind::Sc3);
        assert_eq!(url.host_str().unwrap_or(""), "123.push.ft07.com");
        assert!(url.path().starts_with("/send/"));
        assert!(url.path().ends_with(".send"));
    }

    #[test]
    fn debug_redacts_send_key() {
        let cfg = ServerChanConfig::new("SCTsecret");
        let cfg_dbg = format!("{cfg:?}");
        assert!(!cfg_dbg.contains("SCTsecret"), "{cfg_dbg}");
        assert!(cfg_dbg.contains("<redacted>"), "{cfg_dbg}");
    }

    #[test]
    fn rejects_empty_send_key() {
        let cfg = ServerChanConfig::new("   ");
        let err = ServerChanSink::new(cfg).expect_err("expected invalid config");
        assert!(err.to_string().contains("send_key"), "{err:#}");
    }

    #[test]
    fn redact_url_str_never_leaks_send_key() {
        let cfg = ServerChanConfig::new("SCTsecret");
        let (kind, url) = build_serverchan_url(&cfg.send_key).expect("build url");
        assert!(matches!(kind, ServerChanKind::Turbo | ServerChanKind::Sc3));
        let redacted = redact_url_str(url.as_str());
        assert!(!redacted.contains("SCTsecret"), "{redacted}");
        assert!(redacted.contains("<redacted>"), "{redacted}");
    }
}
