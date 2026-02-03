use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::Event;
use crate::sinks::crypto::hmac_sha256_base64;
use crate::sinks::http::{
    DEFAULT_MAX_RESPONSE_BODY_BYTES, build_http_client, build_http_client_pinned_async,
    parse_and_validate_https_url, read_json_body_limited, redact_url, redact_url_str,
    sanitize_reqwest_error, validate_url_path_prefix,
};
use crate::sinks::text::{TextLimits, format_event_text_limited};
use crate::sinks::{BoxFuture, Sink};

const DINGTALK_ALLOWED_HOSTS: [&str; 1] = ["oapi.dingtalk.com"];

#[non_exhaustive]
#[derive(Clone)]
pub struct DingTalkWebhookConfig {
    pub webhook_url: String,
    pub secret: Option<String>,
    pub timeout: Duration,
    pub max_chars: usize,
    pub enforce_public_ip: bool,
}

impl std::fmt::Debug for DingTalkWebhookConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DingTalkWebhookConfig")
            .field("webhook_url", &redact_url_str(&self.webhook_url))
            .field("secret", &self.secret.as_ref().map(|_| "<redacted>"))
            .field("timeout", &self.timeout)
            .field("max_chars", &self.max_chars)
            .field("enforce_public_ip", &self.enforce_public_ip)
            .finish()
    }
}

impl DingTalkWebhookConfig {
    pub fn new(webhook_url: impl Into<String>) -> Self {
        Self {
            webhook_url: webhook_url.into(),
            secret: None,
            timeout: Duration::from_secs(2),
            max_chars: 4000,
            enforce_public_ip: true,
        }
    }

    #[must_use]
    pub fn with_secret(mut self, secret: impl Into<String>) -> Self {
        self.secret = Some(secret.into());
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

pub struct DingTalkWebhookSink {
    webhook_url: reqwest::Url,
    secret: Option<String>,
    client: reqwest::Client,
    timeout: Duration,
    max_chars: usize,
    enforce_public_ip: bool,
}

impl std::fmt::Debug for DingTalkWebhookSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DingTalkWebhookSink")
            .field("webhook_url", &redact_url(&self.webhook_url))
            .field("secret", &self.secret.as_ref().map(|_| "<redacted>"))
            .field("max_chars", &self.max_chars)
            .finish_non_exhaustive()
    }
}

impl DingTalkWebhookSink {
    pub fn new(config: DingTalkWebhookConfig) -> anyhow::Result<Self> {
        let webhook_url =
            parse_and_validate_https_url(&config.webhook_url, &DINGTALK_ALLOWED_HOSTS)?;
        validate_url_path_prefix(&webhook_url, "/robot/send")?;
        let client = build_http_client(config.timeout)?;

        if let Some(secret) = config.secret.as_deref() {
            if secret.trim().is_empty() {
                return Err(anyhow::anyhow!("dingtalk secret must not be empty"));
            }
        }

        Ok(Self {
            webhook_url,
            secret: config.secret,
            client,
            timeout: config.timeout,
            max_chars: config.max_chars,
            enforce_public_ip: config.enforce_public_ip,
        })
    }

    fn build_payload(event: &Event, max_chars: usize) -> serde_json::Value {
        let text = format_event_text_limited(event, TextLimits::new(max_chars));
        serde_json::json!({
            "msgtype": "text",
            "text": { "content": text },
        })
    }

    fn webhook_url_with_signature(&self) -> anyhow::Result<reqwest::Url> {
        let Some(secret) = self.secret.as_deref() else {
            return Ok(self.webhook_url.clone());
        };

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|err| anyhow::anyhow!("get unix timestamp: {err}"))?
            .as_millis()
            .to_string();

        let string_to_sign = format!("{timestamp}\n{secret}");
        let sign = hmac_sha256_base64(secret, &string_to_sign)?;

        let mut url = self.webhook_url.clone();
        url.query_pairs_mut()
            .append_pair("timestamp", &timestamp)
            .append_pair("sign", &sign);
        Ok(url)
    }
}

impl Sink for DingTalkWebhookSink {
    fn name(&self) -> &'static str {
        "dingtalk"
    }

    fn send<'a>(&'a self, event: &'a Event) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            let url = self.webhook_url_with_signature()?;
            let client = if self.enforce_public_ip {
                build_http_client_pinned_async(self.timeout, url.clone()).await?
            } else {
                self.client.clone()
            };
            let payload = Self::build_payload(event, self.max_chars);

            let resp = client
                .post(url)
                .json(&payload)
                .send()
                .await
                .map_err(|err| {
                    anyhow::anyhow!(
                        "dingtalk webhook request failed ({})",
                        sanitize_reqwest_error(&err)
                    )
                })?;

            let status = resp.status();
            if !status.is_success() {
                return Err(anyhow::anyhow!(
                    "dingtalk webhook http error: {status} (response body omitted)"
                ));
            }

            let Ok(body) = read_json_body_limited(resp, DEFAULT_MAX_RESPONSE_BODY_BYTES).await
            else {
                return Ok(());
            };

            let errcode = body["errcode"].as_i64().unwrap_or(-1);
            if errcode == 0 {
                return Ok(());
            }

            Err(anyhow::anyhow!(
                "dingtalk api error: errcode={errcode} (response body omitted)"
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

        let payload = DingTalkWebhookSink::build_payload(&event, 4000);
        assert_eq!(payload["msgtype"].as_str().unwrap_or(""), "text");
        let text = payload["text"]["content"].as_str().unwrap_or("");
        assert!(text.contains("done"));
        assert!(text.contains("ok"));
        assert!(text.contains("thread_id=t1"));
    }

    #[test]
    fn rejects_non_https_webhook_url() {
        let cfg = DingTalkWebhookConfig::new("http://oapi.dingtalk.com/robot/send?access_token=x");
        let err = DingTalkWebhookSink::new(cfg).expect_err("expected invalid url");
        assert!(err.to_string().contains("https"), "{err:#}");
    }

    #[test]
    fn rejects_unexpected_webhook_host() {
        let cfg = DingTalkWebhookConfig::new("https://example.com/robot/send?access_token=x");
        let err = DingTalkWebhookSink::new(cfg).expect_err("expected invalid host");
        assert!(err.to_string().contains("host is not allowed"), "{err:#}");
    }

    #[test]
    fn rejects_unexpected_webhook_path() {
        let cfg = DingTalkWebhookConfig::new("https://oapi.dingtalk.com/robot/evil?access_token=x");
        let err = DingTalkWebhookSink::new(cfg).expect_err("expected invalid path");
        assert!(err.to_string().contains("path is not allowed"), "{err:#}");
    }

    #[test]
    fn debug_redacts_webhook_url_and_secret() {
        let url = "https://oapi.dingtalk.com/robot/send?access_token=secret_token";
        let cfg = DingTalkWebhookConfig::new(url).with_secret("my_secret");
        let cfg_dbg = format!("{cfg:?}");
        assert!(!cfg_dbg.contains("secret_token"), "{cfg_dbg}");
        assert!(!cfg_dbg.contains("my_secret"), "{cfg_dbg}");
        assert!(cfg_dbg.contains("oapi.dingtalk.com"), "{cfg_dbg}");
        assert!(cfg_dbg.contains("<redacted>"), "{cfg_dbg}");

        let sink = DingTalkWebhookSink::new(cfg).expect("build sink");
        let sink_dbg = format!("{sink:?}");
        assert!(!sink_dbg.contains("secret_token"), "{sink_dbg}");
        assert!(!sink_dbg.contains("my_secret"), "{sink_dbg}");
        assert!(sink_dbg.contains("oapi.dingtalk.com"), "{sink_dbg}");
        assert!(sink_dbg.contains("<redacted>"), "{sink_dbg}");
    }
}
