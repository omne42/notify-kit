use std::time::{SystemTime, UNIX_EPOCH};

use crate::Event;
use crate::sinks::crypto::hmac_sha256_base64;
use crate::sinks::http::{
    DEFAULT_MAX_RESPONSE_BODY_BYTES, build_http_client, parse_and_validate_https_url,
    read_json_body_limited, redact_url, redact_url_str, sanitize_reqwest_error,
    validate_url_path_prefix, validate_url_resolves_to_public_ip,
};
use crate::sinks::text::{TextLimits, format_event_text_limited};
use crate::sinks::{BoxFuture, Sink};

const FEISHU_MAX_CHARS: usize = 4000;

#[derive(Clone)]
pub struct FeishuWebhookConfig {
    pub webhook_url: String,
    pub timeout: std::time::Duration,
}

impl std::fmt::Debug for FeishuWebhookConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeishuWebhookConfig")
            .field("webhook_url", &redact_url_str(&self.webhook_url))
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
    secret: Option<String>,
}

impl std::fmt::Debug for FeishuWebhookSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeishuWebhookSink")
            .field("webhook_url", &redact_url(&self.webhook_url))
            .field("secret", &self.secret.as_ref().map(|_| "<redacted>"))
            .finish_non_exhaustive()
    }
}

impl FeishuWebhookSink {
    pub fn new(config: FeishuWebhookConfig) -> anyhow::Result<Self> {
        Self::new_internal(config, None, false)
    }

    pub fn new_strict(config: FeishuWebhookConfig) -> anyhow::Result<Self> {
        Self::new_internal(config, None, true)
    }

    pub fn new_with_secret(
        config: FeishuWebhookConfig,
        secret: impl Into<String>,
    ) -> anyhow::Result<Self> {
        let secret = secret.into();
        if secret.trim().is_empty() {
            return Err(anyhow::anyhow!("feishu secret must not be empty"));
        }
        Self::new_internal(config, Some(secret), false)
    }

    pub fn new_with_secret_strict(
        config: FeishuWebhookConfig,
        secret: impl Into<String>,
    ) -> anyhow::Result<Self> {
        let secret = secret.into();
        if secret.trim().is_empty() {
            return Err(anyhow::anyhow!("feishu secret must not be empty"));
        }
        Self::new_internal(config, Some(secret), true)
    }

    fn new_internal(
        config: FeishuWebhookConfig,
        secret: Option<String>,
        enforce_public_ip: bool,
    ) -> anyhow::Result<Self> {
        let webhook_url = parse_and_validate_https_url(
            &config.webhook_url,
            &["open.feishu.cn", "open.larksuite.com"],
        )?;
        validate_url_path_prefix(&webhook_url, "/open-apis/bot/v2/hook/")?;
        if enforce_public_ip {
            validate_url_resolves_to_public_ip(&webhook_url)?;
        }
        let client = build_http_client(config.timeout)?;
        Ok(Self {
            webhook_url,
            client,
            secret,
        })
    }

    fn build_payload(
        event: &Event,
        timestamp: Option<&str>,
        sign: Option<&str>,
    ) -> serde_json::Value {
        let mut obj = serde_json::Map::new();
        obj.insert("msg_type".to_string(), serde_json::json!("text"));
        obj.insert(
            "content".to_string(),
            serde_json::json!({
                "text": format_event_text_limited(event, TextLimits::new(FEISHU_MAX_CHARS)),
            }),
        );
        if let Some(timestamp) = timestamp {
            obj.insert("timestamp".to_string(), serde_json::json!(timestamp));
        }
        if let Some(sign) = sign {
            obj.insert("sign".to_string(), serde_json::json!(sign));
        }
        serde_json::Value::Object(obj)
    }
}

impl Sink for FeishuWebhookSink {
    fn name(&self) -> &'static str {
        "feishu"
    }

    fn send<'a>(&'a self, event: &'a Event) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            let (timestamp, sign) = if let Some(secret) = self.secret.as_deref() {
                let timestamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|err| anyhow::anyhow!("get unix timestamp: {err}"))?
                    .as_secs()
                    .to_string();

                let string_to_sign = format!("{timestamp}\n{secret}");
                let sign = hmac_sha256_base64(secret, &string_to_sign)?;

                (Some(timestamp), Some(sign))
            } else {
                (None, None)
            };

            let payload = Self::build_payload(event, timestamp.as_deref(), sign.as_deref());

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
            if !status.is_success() {
                return Err(anyhow::anyhow!(
                    "feishu webhook http error: {status} (response body omitted)"
                ));
            }

            let Ok(body) = read_json_body_limited(resp, DEFAULT_MAX_RESPONSE_BODY_BYTES).await
            else {
                return Ok(());
            };

            let code = body["StatusCode"]
                .as_i64()
                .or_else(|| body["code"].as_i64())
                .unwrap_or(0);
            if code == 0 {
                return Ok(());
            }

            Err(anyhow::anyhow!(
                "feishu api error: code={code} (response body omitted)"
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

        let payload = FeishuWebhookSink::build_payload(&event, None, None);
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
    fn rejects_unexpected_webhook_path() {
        let cfg = FeishuWebhookConfig::new("https://open.feishu.cn/api/x");
        let err = FeishuWebhookSink::new(cfg).expect_err("expected invalid path");
        assert!(err.to_string().contains("path is not allowed"), "{err:#}");
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

    #[test]
    fn builds_payload_with_signature_fields() {
        let event = Event::new("kind", crate::Severity::Info, "title");
        let payload = FeishuWebhookSink::build_payload(&event, Some("123"), Some("sig"));
        assert_eq!(payload["timestamp"].as_str().unwrap_or(""), "123");
        assert_eq!(payload["sign"].as_str().unwrap_or(""), "sig");
    }
}
