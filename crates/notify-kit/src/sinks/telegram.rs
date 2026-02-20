use std::time::Duration;

use crate::Event;
use crate::sinks::http::{
    DEFAULT_MAX_RESPONSE_BODY_BYTES, build_http_client, read_json_body_limited,
    read_text_body_limited, redact_url, send_reqwest,
};
use crate::sinks::text::{TextLimits, format_event_text_limited, truncate_chars};
use crate::sinks::{BoxFuture, Sink};

const TELEGRAM_API_BASE: &str = "https://api.telegram.org";

#[non_exhaustive]
#[derive(Clone)]
pub struct TelegramBotConfig {
    pub bot_token: String,
    pub chat_id: String,
    pub timeout: Duration,
    pub max_chars: usize,
}

impl std::fmt::Debug for TelegramBotConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelegramBotConfig")
            .field("bot_token", &"<redacted>")
            .field("chat_id", &self.chat_id)
            .field("timeout", &self.timeout)
            .field("max_chars", &self.max_chars)
            .finish()
    }
}

impl TelegramBotConfig {
    pub fn new(bot_token: impl Into<String>, chat_id: impl Into<String>) -> Self {
        Self {
            bot_token: bot_token.into(),
            chat_id: chat_id.into(),
            timeout: Duration::from_secs(2),
            max_chars: 4096,
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
}

pub struct TelegramBotSink {
    api_url: reqwest::Url,
    chat_id: String,
    client: reqwest::Client,
    max_chars: usize,
}

impl std::fmt::Debug for TelegramBotSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelegramBotSink")
            .field("api_url", &redact_url(&self.api_url))
            .field("chat_id", &self.chat_id)
            .field("max_chars", &self.max_chars)
            .finish_non_exhaustive()
    }
}

impl TelegramBotSink {
    pub fn new(config: TelegramBotConfig) -> crate::Result<Self> {
        let bot_token = config.bot_token.trim();
        if bot_token.is_empty() {
            return Err(anyhow::anyhow!("telegram bot_token must not be empty").into());
        }
        let chat_id = config.chat_id.trim();
        if chat_id.is_empty() {
            return Err(anyhow::anyhow!("telegram chat_id must not be empty").into());
        }

        let mut api_url = reqwest::Url::parse(TELEGRAM_API_BASE)
            .map_err(|err| anyhow::anyhow!("invalid telegram api base url: {err}"))?;
        let bot_segment = format!("bot{bot_token}");
        api_url
            .path_segments_mut()
            .map_err(|_| anyhow::anyhow!("invalid telegram api base url"))?
            .push(&bot_segment)
            .push("sendMessage");

        let client = build_http_client(config.timeout)?;
        Ok(Self {
            api_url,
            chat_id: chat_id.to_string(),
            client,
            max_chars: config.max_chars,
        })
    }

    fn build_payload(event: &Event, chat_id: &str, max_chars: usize) -> serde_json::Value {
        let text = format_event_text_limited(event, TextLimits::new(max_chars));
        serde_json::json!({
            "chat_id": chat_id,
            "text": text,
            "disable_web_page_preview": true,
        })
    }
}

impl Sink for TelegramBotSink {
    fn name(&self) -> &'static str {
        "telegram"
    }

    fn send<'a>(&'a self, event: &'a Event) -> BoxFuture<'a, crate::Result<()>> {
        Box::pin(async move {
            let payload = Self::build_payload(event, &self.chat_id, self.max_chars);

            let resp = send_reqwest(
                self.client.post(self.api_url.clone()).json(&payload),
                "telegram",
            )
            .await?;

            let status = resp.status();
            if !status.is_success() {
                let body = match read_text_body_limited(resp, DEFAULT_MAX_RESPONSE_BODY_BYTES).await
                {
                    Ok(body) => body,
                    Err(err) => {
                        return Err(anyhow::anyhow!(
                            "telegram http error: {status} (failed to read response body: {err})"
                        )
                        .into());
                    }
                };
                let summary = truncate_chars(body.trim(), 200);
                if summary.is_empty() {
                    return Err(anyhow::anyhow!(
                        "telegram http error: {status} (response body omitted)"
                    )
                    .into());
                }
                return Err(
                    anyhow::anyhow!("telegram http error: {status}, response={summary}").into(),
                );
            }

            let body = read_json_body_limited(resp, DEFAULT_MAX_RESPONSE_BODY_BYTES).await?;

            let ok = body["ok"].as_bool().unwrap_or(false);
            if ok {
                return Ok(());
            }

            let code = body["error_code"].as_i64();
            let description = body["description"].as_str().unwrap_or("");
            let description = truncate_chars(description, 200);
            if let Some(code) = code {
                if !description.is_empty() {
                    return Err(anyhow::anyhow!(
                        "telegram api error: {code}, description={description} (response body omitted)"
                    )
                    .into());
                }
                return Err(
                    anyhow::anyhow!("telegram api error: {code} (response body omitted)").into(),
                );
            }

            if !description.is_empty() {
                return Err(anyhow::anyhow!(
                    "telegram api error: description={description} (response body omitted)"
                )
                .into());
            }

            Err(anyhow::anyhow!("telegram api error (response body omitted)").into())
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

        let payload = TelegramBotSink::build_payload(&event, "123", 4096);
        let text = payload["text"].as_str().unwrap_or("");
        assert!(text.contains("done"));
        assert!(text.contains("ok"));
        assert!(text.contains("thread_id=t1"));
        assert_eq!(payload["chat_id"].as_str().unwrap_or(""), "123");
    }

    #[test]
    fn debug_redacts_bot_token() {
        let cfg = TelegramBotConfig::new("token:secret", "123");
        let cfg_dbg = format!("{cfg:?}");
        assert!(!cfg_dbg.contains("token:secret"), "{cfg_dbg}");
        assert!(cfg_dbg.contains("<redacted>"), "{cfg_dbg}");

        let sink = TelegramBotSink::new(cfg).expect("build sink");
        let sink_dbg = format!("{sink:?}");
        assert!(!sink_dbg.contains("token:secret"), "{sink_dbg}");
        assert!(sink_dbg.contains("api.telegram.org"), "{sink_dbg}");
        assert!(sink_dbg.contains("<redacted>"), "{sink_dbg}");
    }

    #[test]
    fn bot_token_cannot_inject_url_structure() {
        let cfg = TelegramBotConfig::new("a/b?c#d", "123");
        let sink = TelegramBotSink::new(cfg).expect("build sink");
        assert_eq!(sink.api_url.scheme(), "https");
        assert_eq!(sink.api_url.host_str().unwrap_or(""), "api.telegram.org");
        assert!(sink.api_url.query().is_none(), "query must be none");
        assert!(sink.api_url.fragment().is_none(), "fragment must be none");

        let path = sink.api_url.path();
        assert!(path.starts_with("/bot"), "{path}");
        assert!(path.ends_with("/sendMessage"), "{path}");
    }

    #[test]
    fn trims_bot_token_and_chat_id() {
        let cfg = TelegramBotConfig::new(" token:secret ", " 123 ");
        let sink = TelegramBotSink::new(cfg).expect("build sink");
        assert_eq!(sink.chat_id, "123");
        assert!(
            sink.api_url.path().starts_with("/bot"),
            "{}",
            sink.api_url.path()
        );
        assert!(
            sink.api_url.path().ends_with("/sendMessage"),
            "{}",
            sink.api_url.path()
        );
        assert!(
            !sink.api_url.as_str().contains("%20"),
            "{}",
            sink.api_url.as_str()
        );
    }
}
