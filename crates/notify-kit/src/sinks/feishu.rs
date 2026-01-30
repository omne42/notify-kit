use crate::Event;
use crate::sinks::{BoxFuture, Sink};

#[derive(Debug, Clone)]
pub struct FeishuWebhookConfig {
    pub webhook_url: String,
    pub timeout: std::time::Duration,
}

impl FeishuWebhookConfig {
    pub fn new(webhook_url: impl Into<String>) -> Self {
        Self {
            webhook_url: webhook_url.into(),
            timeout: std::time::Duration::from_secs(2),
        }
    }
}

#[derive(Debug)]
pub struct FeishuWebhookSink {
    webhook_url: String,
    client: reqwest::Client,
}

impl FeishuWebhookSink {
    pub fn new(config: FeishuWebhookConfig) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|err| anyhow::anyhow!("build reqwest client: {err}"))?;
        Ok(Self {
            webhook_url: config.webhook_url,
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
                .map_err(|err| anyhow::anyhow!("feishu webhook request: {err}"))?;

            let status = resp.status();
            if status.is_success() {
                return Ok(());
            }

            let body = resp
                .text()
                .await
                .unwrap_or_else(|_| "<read body failed>".to_string());
            Err(anyhow::anyhow!(
                "feishu webhook http error: {status} body={body}"
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
}
