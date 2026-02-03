use std::time::Duration;

use crate::Event;
use crate::sinks::http::{
    DEFAULT_MAX_RESPONSE_BODY_BYTES, build_http_client, build_http_client_pinned_async,
    parse_and_validate_https_url, read_json_body_limited, redact_url, sanitize_reqwest_error,
    validate_url_path_prefix,
};
use crate::sinks::text::{TextLimits, format_event_body_and_tags_limited, truncate_chars};
use crate::sinks::{BoxFuture, Sink};

const PUSHPLUS_ALLOWED_HOSTS: [&str; 1] = ["www.pushplus.plus"];

#[non_exhaustive]
#[derive(Clone)]
pub struct PushPlusConfig {
    pub token: String,
    pub channel: Option<String>,
    pub template: Option<String>,
    pub topic: Option<String>,
    pub timeout: Duration,
    pub max_chars: usize,
    pub enforce_public_ip: bool,
}

impl std::fmt::Debug for PushPlusConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PushPlusConfig")
            .field("token", &"<redacted>")
            .field("channel", &self.channel)
            .field("template", &self.template)
            .field("topic", &self.topic)
            .field("timeout", &self.timeout)
            .field("max_chars", &self.max_chars)
            .field("enforce_public_ip", &self.enforce_public_ip)
            .finish()
    }
}

impl PushPlusConfig {
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            channel: None,
            template: Some("txt".to_string()),
            topic: None,
            timeout: Duration::from_secs(2),
            max_chars: 16 * 1024,
            enforce_public_ip: true,
        }
    }

    #[must_use]
    pub fn with_channel(mut self, channel: impl Into<String>) -> Self {
        self.channel = Some(channel.into());
        self
    }

    #[must_use]
    pub fn with_template(mut self, template: impl Into<String>) -> Self {
        self.template = Some(template.into());
        self
    }

    #[must_use]
    pub fn without_template(mut self) -> Self {
        self.template = None;
        self
    }

    #[must_use]
    pub fn with_topic(mut self, topic: impl Into<String>) -> Self {
        self.topic = Some(topic.into());
        self
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

pub struct PushPlusSink {
    api_url: reqwest::Url,
    token: String,
    channel: Option<String>,
    template: Option<String>,
    topic: Option<String>,
    client: reqwest::Client,
    timeout: Duration,
    max_chars: usize,
    enforce_public_ip: bool,
}

impl std::fmt::Debug for PushPlusSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PushPlusSink")
            .field("api_url", &redact_url(&self.api_url))
            .field("token", &"<redacted>")
            .field("channel", &self.channel)
            .field("template", &self.template)
            .field("topic", &self.topic)
            .field("max_chars", &self.max_chars)
            .field("enforce_public_ip", &self.enforce_public_ip)
            .finish_non_exhaustive()
    }
}

impl PushPlusSink {
    pub fn new(config: PushPlusConfig) -> anyhow::Result<Self> {
        if config.token.trim().is_empty() {
            return Err(anyhow::anyhow!("pushplus token must not be empty"));
        }

        let api_url = parse_and_validate_https_url(
            "https://www.pushplus.plus/send",
            &PUSHPLUS_ALLOWED_HOSTS,
        )?;
        validate_url_path_prefix(&api_url, "/send")?;

        let client = build_http_client(config.timeout)?;
        Ok(Self {
            api_url,
            token: config.token,
            channel: config.channel,
            template: config.template,
            topic: config.topic,
            client,
            timeout: config.timeout,
            max_chars: config.max_chars,
            enforce_public_ip: config.enforce_public_ip,
        })
    }

    fn build_payload(
        event: &Event,
        token: &str,
        channel: Option<&str>,
        template: Option<&str>,
        topic: Option<&str>,
        max_chars: usize,
    ) -> serde_json::Value {
        let title = truncate_chars(&event.title, 256);
        let content = format_event_body_and_tags_limited(event, TextLimits::new(max_chars));

        let mut obj = serde_json::Map::new();
        obj.insert("token".to_string(), serde_json::json!(token));
        obj.insert("title".to_string(), serde_json::json!(title));
        obj.insert("content".to_string(), serde_json::json!(content));

        if let Some(channel) = channel {
            let channel = channel.trim();
            if !channel.is_empty() {
                obj.insert("channel".to_string(), serde_json::json!(channel));
            }
        }
        if let Some(template) = template {
            let template = template.trim();
            if !template.is_empty() {
                obj.insert("template".to_string(), serde_json::json!(template));
            }
        }
        if let Some(topic) = topic {
            let topic = topic.trim();
            if !topic.is_empty() {
                obj.insert("topic".to_string(), serde_json::json!(topic));
            }
        }

        serde_json::Value::Object(obj)
    }
}

impl Sink for PushPlusSink {
    fn name(&self) -> &'static str {
        "pushplus"
    }

    fn send<'a>(&'a self, event: &'a Event) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            let client = if self.enforce_public_ip {
                build_http_client_pinned_async(self.timeout, self.api_url.clone()).await?
            } else {
                self.client.clone()
            };

            let payload = Self::build_payload(
                event,
                &self.token,
                self.channel.as_deref(),
                self.template.as_deref(),
                self.topic.as_deref(),
                self.max_chars,
            );

            let resp = client
                .post(self.api_url.clone())
                .json(&payload)
                .send()
                .await
                .map_err(|err| {
                    anyhow::anyhow!("pushplus request failed ({})", sanitize_reqwest_error(&err))
                })?;

            let status = resp.status();
            if !status.is_success() {
                return Err(anyhow::anyhow!(
                    "pushplus http error: {status} (response body omitted)"
                ));
            }

            let body = read_json_body_limited(resp, DEFAULT_MAX_RESPONSE_BODY_BYTES).await?;

            let code = body["code"].as_i64().unwrap_or(-1);
            if code == 200 {
                return Ok(());
            }

            let msg = body["msg"].as_str().unwrap_or("");
            let msg = truncate_chars(msg, 200);
            Err(anyhow::anyhow!(
                "pushplus api error: code={code}, msg={msg} (response body omitted)"
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Severity;

    #[test]
    fn builds_expected_payload() {
        let event = Event::new("turn_completed", Severity::Success, "done")
            .with_body("ok")
            .with_tag("thread_id", "t1");

        let payload =
            PushPlusSink::build_payload(&event, "tok", None, Some("txt"), None, 16 * 1024);
        assert_eq!(payload["token"].as_str().unwrap_or(""), "tok");
        assert_eq!(payload["title"].as_str().unwrap_or(""), "done");
        let content = payload["content"].as_str().unwrap_or("");
        assert!(content.contains("ok"));
        assert!(content.contains("thread_id=t1"));
        assert_eq!(payload["template"].as_str().unwrap_or(""), "txt");
    }

    #[test]
    fn debug_redacts_token() {
        let cfg = PushPlusConfig::new("tok_secret");
        let cfg_dbg = format!("{cfg:?}");
        assert!(!cfg_dbg.contains("tok_secret"), "{cfg_dbg}");
        assert!(cfg_dbg.contains("<redacted>"), "{cfg_dbg}");

        let sink = PushPlusSink::new(cfg).expect("build sink");
        let sink_dbg = format!("{sink:?}");
        assert!(!sink_dbg.contains("tok_secret"), "{sink_dbg}");
        assert!(sink_dbg.contains("pushplus.plus"), "{sink_dbg}");
        assert!(sink_dbg.contains("<redacted>"), "{sink_dbg}");
    }

    #[test]
    fn rejects_empty_token() {
        let cfg = PushPlusConfig::new("   ");
        let err = PushPlusSink::new(cfg).expect_err("expected invalid config");
        assert!(err.to_string().contains("token"), "{err:#}");
    }
}
