use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::Event;
use crate::sinks::crypto::hmac_sha256_base64;
use crate::sinks::http::{
    DEFAULT_MAX_RESPONSE_BODY_BYTES, build_http_client, parse_and_validate_https_url,
    read_json_body_limited, read_text_body_limited, redact_url, redact_url_str, select_http_client,
    send_reqwest, validate_url_path_prefix,
};
use crate::sinks::text::{TextLimits, format_event_text_limited, truncate_chars};
use crate::sinks::{BoxFuture, Sink};

const FEISHU_MAX_CHARS: usize = 4000;

#[non_exhaustive]
#[derive(Clone)]
pub struct FeishuWebhookConfig {
    pub webhook_url: String,
    pub timeout: Duration,
    pub max_chars: usize,
    pub enforce_public_ip: bool,
}

impl std::fmt::Debug for FeishuWebhookConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeishuWebhookConfig")
            .field("webhook_url", &redact_url_str(&self.webhook_url))
            .field("timeout", &self.timeout)
            .field("max_chars", &self.max_chars)
            .field("enforce_public_ip", &self.enforce_public_ip)
            .finish()
    }
}

impl FeishuWebhookConfig {
    pub fn new(webhook_url: impl Into<String>) -> Self {
        Self {
            webhook_url: webhook_url.into(),
            timeout: Duration::from_secs(2),
            max_chars: FEISHU_MAX_CHARS,
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

pub struct FeishuWebhookSink {
    webhook_url: reqwest::Url,
    client: reqwest::Client,
    timeout: Duration,
    secret: Option<String>,
    max_chars: usize,
    enforce_public_ip: bool,
}

impl std::fmt::Debug for FeishuWebhookSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeishuWebhookSink")
            .field("webhook_url", &redact_url(&self.webhook_url))
            .field("secret", &self.secret.as_ref().map(|_| "<redacted>"))
            .field("max_chars", &self.max_chars)
            .field("enforce_public_ip", &self.enforce_public_ip)
            .finish_non_exhaustive()
    }
}

impl FeishuWebhookSink {
    pub fn new(config: FeishuWebhookConfig) -> crate::Result<Self> {
        Self::new_internal(config, None, false)
    }

    pub fn new_strict(config: FeishuWebhookConfig) -> crate::Result<Self> {
        Self::new_internal(config, None, true)
    }

    pub async fn new_strict_async(config: FeishuWebhookConfig) -> crate::Result<Self> {
        Self::new_internal_async(config, None, true).await
    }

    pub fn new_with_secret(
        config: FeishuWebhookConfig,
        secret: impl Into<String>,
    ) -> crate::Result<Self> {
        let secret = normalize_secret(secret)?;
        Self::new_internal(config, Some(secret), false)
    }

    pub fn new_with_secret_strict(
        config: FeishuWebhookConfig,
        secret: impl Into<String>,
    ) -> crate::Result<Self> {
        let secret = normalize_secret(secret)?;
        Self::new_internal(config, Some(secret), true)
    }

    pub async fn new_with_secret_strict_async(
        config: FeishuWebhookConfig,
        secret: impl Into<String>,
    ) -> crate::Result<Self> {
        let secret = normalize_secret(secret)?;
        Self::new_internal_async(config, Some(secret), true).await
    }

    fn new_internal(
        config: FeishuWebhookConfig,
        secret: Option<String>,
        validate_public_ip_at_construction: bool,
    ) -> crate::Result<Self> {
        let enforce_public_ip = config.enforce_public_ip;
        if validate_public_ip_at_construction && !enforce_public_ip {
            return Err(anyhow::anyhow!("feishu strict mode requires public ip check").into());
        }
        let webhook_url = parse_and_validate_https_url(
            &config.webhook_url,
            &["open.feishu.cn", "open.larksuite.com"],
        )?;
        validate_url_path_prefix(&webhook_url, "/open-apis/bot/v2/hook/")?;
        let client = build_http_client(config.timeout)?;
        if validate_public_ip_at_construction {
            if tokio::runtime::Handle::try_current().is_ok() {
                return Err(anyhow::anyhow!(
                    "feishu strict constructor cannot run inside tokio runtime; use new_strict_async/new_with_secret_strict_async"
                )
                .into());
            }
            Self::validate_public_ip_at_construction_sync(&client, config.timeout, &webhook_url)?;
        }
        Ok(Self {
            webhook_url,
            client,
            timeout: config.timeout,
            secret,
            max_chars: config.max_chars,
            enforce_public_ip,
        })
    }

    async fn new_internal_async(
        config: FeishuWebhookConfig,
        secret: Option<String>,
        validate_public_ip_at_construction: bool,
    ) -> crate::Result<Self> {
        let enforce_public_ip = config.enforce_public_ip;
        if validate_public_ip_at_construction && !enforce_public_ip {
            return Err(anyhow::anyhow!("feishu strict mode requires public ip check").into());
        }
        let webhook_url = parse_and_validate_https_url(
            &config.webhook_url,
            &["open.feishu.cn", "open.larksuite.com"],
        )?;
        validate_url_path_prefix(&webhook_url, "/open-apis/bot/v2/hook/")?;
        let client = build_http_client(config.timeout)?;
        if validate_public_ip_at_construction {
            select_http_client(&client, config.timeout, &webhook_url, true)
                .await
                .map(|_| ())?;
        }
        Ok(Self {
            webhook_url,
            client,
            timeout: config.timeout,
            secret,
            max_chars: config.max_chars,
            enforce_public_ip,
        })
    }

    fn build_payload(
        event: &Event,
        max_chars: usize,
        timestamp: Option<&str>,
        sign: Option<&str>,
    ) -> serde_json::Value {
        let mut obj = serde_json::Map::new();
        obj.insert("msg_type".to_string(), serde_json::json!("text"));
        obj.insert(
            "content".to_string(),
            serde_json::json!({
                "text": format_event_text_limited(event, TextLimits::new(max_chars)),
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

    fn ensure_success_response(body: &serde_json::Value) -> crate::Result<()> {
        let Some(code) = body["StatusCode"]
            .as_i64()
            .or_else(|| body["code"].as_i64())
        else {
            return Err(anyhow::anyhow!(
                "feishu api error: missing status code (response body omitted)"
            )
            .into());
        };

        if code == 0 {
            return Ok(());
        }

        Err(anyhow::anyhow!("feishu api error: code={code} (response body omitted)").into())
    }

    fn validate_public_ip_at_construction_sync(
        client: &reqwest::Client,
        timeout: Duration,
        webhook_url: &reqwest::Url,
    ) -> crate::Result<()> {
        let client = client.clone();
        let webhook_url = webhook_url.clone();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|err| anyhow::anyhow!("build tokio runtime: {err}"))?;
        rt.block_on(async move {
            select_http_client(&client, timeout, &webhook_url, true)
                .await
                .map(|_| ())
        })
    }
}

fn normalize_secret(secret: impl Into<String>) -> crate::Result<String> {
    let secret = secret.into();
    let secret = secret.trim();
    if secret.is_empty() {
        return Err(anyhow::anyhow!("feishu secret must not be empty").into());
    }
    Ok(secret.to_string())
}

impl Sink for FeishuWebhookSink {
    fn name(&self) -> &'static str {
        "feishu"
    }

    fn send<'a>(&'a self, event: &'a Event) -> BoxFuture<'a, crate::Result<()>> {
        Box::pin(async move {
            let client = select_http_client(
                &self.client,
                self.timeout,
                &self.webhook_url,
                self.enforce_public_ip,
            )
            .await?;
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

            let payload =
                Self::build_payload(event, self.max_chars, timestamp.as_deref(), sign.as_deref());

            let resp = send_reqwest(
                client.post(self.webhook_url.clone()).json(&payload),
                "feishu webhook",
            )
            .await?;

            let status = resp.status();
            if !status.is_success() {
                let body = match read_text_body_limited(resp, DEFAULT_MAX_RESPONSE_BODY_BYTES).await
                {
                    Ok(body) => body,
                    Err(err) => {
                        return Err(anyhow::anyhow!(
                            "feishu webhook http error: {status} (failed to read response body: {err})"
                        )
                        .into());
                    }
                };
                let summary = truncate_chars(body.trim(), 200);
                if summary.is_empty() {
                    return Err(anyhow::anyhow!(
                        "feishu webhook http error: {status} (response body omitted)"
                    )
                    .into());
                }
                return Err(anyhow::anyhow!(
                    "feishu webhook http error: {status}, response={summary}"
                )
                .into());
            }

            let body = read_json_body_limited(resp, DEFAULT_MAX_RESPONSE_BODY_BYTES).await?;
            Self::ensure_success_response(&body)
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

        let payload = FeishuWebhookSink::build_payload(&event, FEISHU_MAX_CHARS, None, None);
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
    fn strict_requires_public_ip_check() {
        let cfg = FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/x")
            .with_public_ip_check(false);
        let err = FeishuWebhookSink::new_strict(cfg).expect_err("expected strict validation");
        assert!(err.to_string().contains("public ip"), "{err:#}");
    }

    #[test]
    fn strict_sync_constructor_rejects_inside_runtime() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");
        rt.block_on(async {
            let cfg = FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/x");
            let err =
                FeishuWebhookSink::new_strict(cfg).expect_err("expected runtime constructor error");
            assert!(err.to_string().contains("new_strict_async"), "{err:#}");
        });
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
        let payload =
            FeishuWebhookSink::build_payload(&event, FEISHU_MAX_CHARS, Some("123"), Some("sig"));
        assert_eq!(payload["timestamp"].as_str().unwrap_or(""), "123");
        assert_eq!(payload["sign"].as_str().unwrap_or(""), "sig");
    }

    #[test]
    fn trims_secret() {
        let cfg = FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/x");
        let sink =
            FeishuWebhookSink::new_with_secret(cfg, "  my_secret  ").expect("build secret sink");
        assert_eq!(sink.secret.as_deref(), Some("my_secret"));
    }

    #[test]
    fn payload_respects_max_chars() {
        let event = Event::new("kind", crate::Severity::Info, "title").with_body("x".repeat(100));
        let payload = FeishuWebhookSink::build_payload(&event, 10, None, None);
        let text = payload["content"]["text"].as_str().unwrap_or("");
        assert!(text.chars().count() <= 10, "{text}");
        assert!(text.ends_with("..."), "{text}");
    }

    #[test]
    fn response_requires_explicit_success_code() {
        let body = serde_json::json!({});
        let err =
            FeishuWebhookSink::ensure_success_response(&body).expect_err("expected missing code");
        assert!(err.to_string().contains("missing status code"), "{err:#}");
    }

    #[test]
    fn response_accepts_zero_code() {
        let body = serde_json::json!({ "StatusCode": 0 });
        FeishuWebhookSink::ensure_success_response(&body).expect("expected success");

        let body = serde_json::json!({ "code": 0 });
        FeishuWebhookSink::ensure_success_response(&body).expect("expected success");
    }
}
