use crate::Event;
use crate::sinks::{BoxFuture, Sink};

#[derive(Clone)]
pub struct FeishuWebhookConfig {
    pub webhook_url: String,
    pub timeout: std::time::Duration,
}

impl std::fmt::Debug for FeishuWebhookConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeishuWebhookConfig")
            .field("webhook_url", &redact_webhook_url_str(&self.webhook_url))
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl FeishuWebhookConfig {
    pub fn new(webhook_url: impl Into<String>) -> Self {
        Self {
            webhook_url: webhook_url.into(),
            timeout: std::time::Duration::from_secs(2),
        }
    }
}

pub struct FeishuWebhookSink {
    webhook_url: reqwest::Url,
    client: reqwest::Client,
}

impl std::fmt::Debug for FeishuWebhookSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeishuWebhookSink")
            .field("webhook_url", &redact_webhook_url(&self.webhook_url))
            .finish_non_exhaustive()
    }
}

impl FeishuWebhookSink {
    pub fn new(config: FeishuWebhookConfig) -> anyhow::Result<Self> {
        let webhook_url = parse_and_validate_webhook_url(&config.webhook_url)?;
        let client = reqwest::Client::builder()
            .timeout(config.timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|err| anyhow::anyhow!("build reqwest client: {err}"))?;
        Ok(Self {
            webhook_url,
            client,
        })
    }

    fn build_text(event: &Event) -> String {
        let mut out = String::new();
        out.push_str(&event.title);

        if let Some(body) = event.body.as_deref() {
            let body = body.trim();
            if !body.is_empty() {
                out.push('\n');
                out.push_str(body);
            }
        }

        for (k, v) in &event.tags {
            out.push('\n');
            out.push_str(k);
            out.push('=');
            out.push_str(v);
        }

        out
    }

    fn build_payload(event: &Event) -> serde_json::Value {
        serde_json::json!({
            "msg_type": "text",
            "content": {
                "text": Self::build_text(event),
            },
        })
    }
}

fn parse_and_validate_webhook_url(webhook_url: &str) -> anyhow::Result<reqwest::Url> {
    let url = reqwest::Url::parse(webhook_url)
        .map_err(|err| anyhow::anyhow!("invalid webhook url: {err}"))?;

    if url.scheme() != "https" {
        return Err(anyhow::anyhow!("webhook url must use https"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(anyhow::anyhow!("webhook url must not contain credentials"));
    }

    let Some(host) = url.host_str() else {
        return Err(anyhow::anyhow!("webhook url must have a host"));
    };
    if host.eq_ignore_ascii_case("localhost") || host.parse::<std::net::IpAddr>().is_ok() {
        return Err(anyhow::anyhow!("webhook url host is not allowed"));
    }

    let allowed_hosts = ["open.feishu.cn", "open.larksuite.com"];
    if !allowed_hosts
        .iter()
        .any(|allowed| host.eq_ignore_ascii_case(allowed))
    {
        return Err(anyhow::anyhow!("webhook url host is not allowed"));
    }

    if let Some(port) = url.port() {
        if port != 443 {
            return Err(anyhow::anyhow!("webhook url port is not allowed"));
        }
    }

    Ok(url)
}

fn redact_webhook_url_str(webhook_url: &str) -> String {
    let Ok(url) = reqwest::Url::parse(webhook_url) else {
        return "<redacted>".to_string();
    };
    redact_webhook_url(&url)
}

fn redact_webhook_url(url: &reqwest::Url) -> String {
    match (url.scheme(), url.host_str()) {
        (scheme, Some(host)) => format!("{scheme}://{host}/<redacted>"),
        _ => "<redacted>".to_string(),
    }
}

fn sanitize_reqwest_error(err: &reqwest::Error) -> &'static str {
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

impl Sink for FeishuWebhookSink {
    fn name(&self) -> &'static str {
        "feishu"
    }

    fn send<'a>(&'a self, event: &'a Event) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            let payload = Self::build_payload(event);

            let resp = self
                .client
                .post(self.webhook_url.clone())
                .json(&payload)
                .send()
                .await
                .map_err(|err| {
                    anyhow::anyhow!(
                        "feishu webhook request failed ({})",
                        sanitize_reqwest_error(&err)
                    )
                })?;

            let status = resp.status();
            if status.is_success() {
                return Ok(());
            }

            Err(anyhow::anyhow!(
                "feishu webhook http error: {status} (response body omitted)"
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_expected_payload() {
        let event = Event::new("turn_completed", crate::Severity::Success, "done")
            .with_body("ok")
            .with_tag("thread_id", "t1");

        let payload = FeishuWebhookSink::build_payload(&event);
        assert_eq!(payload["msg_type"].as_str().unwrap_or(""), "text");
        let text = payload["content"]["text"].as_str().unwrap_or("");
        assert!(text.contains("done"));
        assert!(text.contains("ok"));
        assert!(text.contains("thread_id=t1"));
    }

    #[test]
    fn rejects_non_https_webhook_url() {
        let cfg = FeishuWebhookConfig::new("http://open.feishu.cn/open-apis/bot/v2/hook/x");
        let err = FeishuWebhookSink::new(cfg).expect_err("expected invalid url");
        assert!(err.to_string().contains("https"), "{err:#}");
    }

    #[test]
    fn rejects_unexpected_webhook_host() {
        let cfg = FeishuWebhookConfig::new("https://example.com/open-apis/bot/v2/hook/x");
        let err = FeishuWebhookSink::new(cfg).expect_err("expected invalid host");
        assert!(err.to_string().contains("host is not allowed"), "{err:#}");
    }

    #[test]
    fn debug_redacts_webhook_url() {
        let url = "https://open.feishu.cn/open-apis/bot/v2/hook/secret_token";
        let cfg = FeishuWebhookConfig::new(url);
        let cfg_dbg = format!("{cfg:?}");
        assert!(!cfg_dbg.contains("secret_token"), "{cfg_dbg}");
        assert!(cfg_dbg.contains("open.feishu.cn"), "{cfg_dbg}");
        assert!(cfg_dbg.contains("<redacted>"), "{cfg_dbg}");

        let sink = FeishuWebhookSink::new(cfg).expect("build sink");
        let sink_dbg = format!("{sink:?}");
        assert!(!sink_dbg.contains("secret_token"), "{sink_dbg}");
        assert!(sink_dbg.contains("open.feishu.cn"), "{sink_dbg}");
        assert!(sink_dbg.contains("<redacted>"), "{sink_dbg}");
    }
}
