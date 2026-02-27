use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;

use crate::Event;
use crate::sinks::crypto::hmac_sha256_base64;
use crate::sinks::http::{
    DEFAULT_MAX_RESPONSE_BODY_BYTES, build_http_client, parse_and_validate_https_url,
    parse_and_validate_https_url_basic, read_json_body_limited, read_text_body_limited, redact_url,
    redact_url_str, select_http_client, send_reqwest, validate_url_path_prefix,
};
use crate::sinks::markdown::{Inline as MarkdownInline, parse_markdown_lines};
use crate::sinks::text::{TextLimits, format_event_text_limited, truncate_chars};
use crate::sinks::{BoxFuture, Sink};

const FEISHU_MAX_CHARS: usize = 4000;
const FEISHU_DEFAULT_IMAGE_UPLOAD_MAX_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Clone)]
struct FeishuAppCredentials {
    app_id: String,
    app_secret: String,
}

#[derive(Debug, Clone)]
struct AccessTokenCache {
    token: String,
    expires_at: Instant,
}

#[derive(Debug)]
struct LoadedImage {
    bytes: Vec<u8>,
    file_name: String,
    content_type: String,
}

#[non_exhaustive]
#[derive(Clone)]
pub struct FeishuWebhookConfig {
    pub webhook_url: String,
    pub timeout: Duration,
    pub max_chars: usize,
    pub enforce_public_ip: bool,
    pub enable_markdown_rich_text: bool,
    pub image_upload_max_bytes: usize,
    pub app_id: Option<String>,
    pub app_secret: Option<String>,
}

impl std::fmt::Debug for FeishuWebhookConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeishuWebhookConfig")
            .field("webhook_url", &redact_url_str(&self.webhook_url))
            .field("timeout", &self.timeout)
            .field("max_chars", &self.max_chars)
            .field("enforce_public_ip", &self.enforce_public_ip)
            .field("enable_markdown_rich_text", &self.enable_markdown_rich_text)
            .field("image_upload_max_bytes", &self.image_upload_max_bytes)
            .field("app_id", &self.app_id.as_ref().map(|_| "<redacted>"))
            .field(
                "app_secret",
                &self.app_secret.as_ref().map(|_| "<redacted>"),
            )
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
            enable_markdown_rich_text: true,
            image_upload_max_bytes: FEISHU_DEFAULT_IMAGE_UPLOAD_MAX_BYTES,
            app_id: None,
            app_secret: None,
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

    #[must_use]
    pub fn with_markdown_rich_text(mut self, enable: bool) -> Self {
        self.enable_markdown_rich_text = enable;
        self
    }

    #[must_use]
    pub fn with_image_upload_max_bytes(mut self, max_bytes: usize) -> Self {
        self.image_upload_max_bytes = max_bytes;
        self
    }

    #[must_use]
    pub fn with_app_credentials(
        mut self,
        app_id: impl Into<String>,
        app_secret: impl Into<String>,
    ) -> Self {
        self.app_id = Some(app_id.into());
        self.app_secret = Some(app_secret.into());
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
    enable_markdown_rich_text: bool,
    image_upload_max_bytes: usize,
    app_credentials: Option<FeishuAppCredentials>,
    tenant_access_token: tokio::sync::Mutex<Option<AccessTokenCache>>,
}

impl std::fmt::Debug for FeishuWebhookSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeishuWebhookSink")
            .field("webhook_url", &redact_url(&self.webhook_url))
            .field("secret", &self.secret.as_ref().map(|_| "<redacted>"))
            .field("max_chars", &self.max_chars)
            .field("enforce_public_ip", &self.enforce_public_ip)
            .field("enable_markdown_rich_text", &self.enable_markdown_rich_text)
            .field("image_upload_max_bytes", &self.image_upload_max_bytes)
            .field(
                "app_credentials",
                &self.app_credentials.as_ref().map(|_| "<redacted>"),
            )
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

        let app_credentials = normalize_app_credentials(config.app_id, config.app_secret)?;
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
            enable_markdown_rich_text: config.enable_markdown_rich_text,
            image_upload_max_bytes: config.image_upload_max_bytes,
            app_credentials,
            tenant_access_token: tokio::sync::Mutex::new(None),
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

        let app_credentials = normalize_app_credentials(config.app_id, config.app_secret)?;
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
            enable_markdown_rich_text: config.enable_markdown_rich_text,
            image_upload_max_bytes: config.image_upload_max_bytes,
            app_credentials,
            tenant_access_token: tokio::sync::Mutex::new(None),
        })
    }

    fn base_payload(
        timestamp: Option<&str>,
        sign: Option<&str>,
    ) -> serde_json::Map<String, serde_json::Value> {
        let mut obj = serde_json::Map::with_capacity(6);
        if let Some(timestamp) = timestamp {
            obj.insert("timestamp".to_string(), serde_json::json!(timestamp));
        }
        if let Some(sign) = sign {
            obj.insert("sign".to_string(), serde_json::json!(sign));
        }
        obj
    }

    fn build_text_payload(
        event: &Event,
        max_chars: usize,
        timestamp: Option<&str>,
        sign: Option<&str>,
    ) -> serde_json::Value {
        let mut obj = Self::base_payload(timestamp, sign);
        obj.insert("msg_type".to_string(), serde_json::json!("text"));
        obj.insert(
            "content".to_string(),
            serde_json::json!({
                "text": format_event_text_limited(event, TextLimits::new(max_chars)),
            }),
        );
        serde_json::Value::Object(obj)
    }

    async fn build_payload(
        &self,
        event: &Event,
        timestamp: Option<&str>,
        sign: Option<&str>,
    ) -> crate::Result<serde_json::Value> {
        if !self.enable_markdown_rich_text {
            return Ok(Self::build_text_payload(
                event,
                self.max_chars,
                timestamp,
                sign,
            ));
        }

        let Some(body) = event
            .body
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        else {
            return Ok(Self::build_text_payload(
                event,
                self.max_chars,
                timestamp,
                sign,
            ));
        };

        let markdown_lines = parse_markdown_lines(body);
        if markdown_lines.is_empty() {
            return Ok(Self::build_text_payload(
                event,
                self.max_chars,
                timestamp,
                sign,
            ));
        }

        let image_keys = self.resolve_image_keys(&markdown_lines).await;

        let mut content_rows: Vec<serde_json::Value> = Vec::new();
        let mut remaining = self.max_chars;

        for line in markdown_lines {
            let mut row: Vec<serde_json::Value> = Vec::new();
            for inline in line.inlines {
                match inline {
                    MarkdownInline::Text(text) => {
                        let text = Self::take_text_budget(&text, &mut remaining);
                        if text.is_empty() {
                            continue;
                        }
                        row.push(serde_json::json!({
                            "tag": "text",
                            "text": text,
                        }));
                    }
                    MarkdownInline::Link { text, href } => {
                        let display = if text.trim().is_empty() {
                            href.clone()
                        } else {
                            text
                        };
                        let display = Self::take_text_budget(&display, &mut remaining);
                        if display.is_empty() {
                            continue;
                        }
                        row.push(serde_json::json!({
                            "tag": "a",
                            "text": display,
                            "href": href,
                        }));
                    }
                    MarkdownInline::Image { alt, src } => {
                        if let Some(image_key) = image_keys.get(&src).and_then(|v| v.clone()) {
                            row.push(serde_json::json!({
                                "tag": "img",
                                "image_key": image_key,
                            }));
                            continue;
                        }

                        let fallback = if alt.trim().is_empty() {
                            format!("[image] {src}")
                        } else {
                            format!("[image:{alt}] {src}")
                        };
                        let fallback = Self::take_text_budget(&fallback, &mut remaining);
                        if fallback.is_empty() {
                            continue;
                        }
                        row.push(serde_json::json!({
                            "tag": "text",
                            "text": fallback,
                        }));
                    }
                }
            }
            if !row.is_empty() {
                content_rows.push(serde_json::Value::Array(row));
            }
            if remaining == 0 {
                break;
            }
        }

        for (k, v) in &event.tags {
            if remaining == 0 {
                break;
            }
            let tag_line = format!("{k}={v}");
            let text = Self::take_text_budget(&tag_line, &mut remaining);
            if text.is_empty() {
                break;
            }
            content_rows.push(serde_json::json!([
                {
                    "tag": "text",
                    "text": text,
                }
            ]));
        }

        if content_rows.is_empty() {
            return Ok(Self::build_text_payload(
                event,
                self.max_chars,
                timestamp,
                sign,
            ));
        }

        let title = truncate_chars(event.title.trim(), 256);
        let mut obj = Self::base_payload(timestamp, sign);
        obj.insert("msg_type".to_string(), serde_json::json!("post"));
        obj.insert(
            "content".to_string(),
            serde_json::json!({
                "post": {
                    "zh_cn": {
                        "title": title,
                        "content": content_rows,
                    }
                }
            }),
        );

        Ok(serde_json::Value::Object(obj))
    }

    fn take_text_budget(input: &str, remaining: &mut usize) -> String {
        if *remaining == 0 || input.is_empty() {
            return String::new();
        }

        let taken = truncate_chars(input, *remaining);
        let taken_chars = taken.chars().count();
        if taken_chars >= *remaining {
            *remaining = 0;
        } else {
            *remaining -= taken_chars;
        }
        taken
    }

    async fn resolve_image_keys(
        &self,
        markdown_lines: &[crate::sinks::markdown::Line],
    ) -> HashMap<String, Option<String>> {
        let mut urls = BTreeSet::new();
        for line in markdown_lines {
            for inline in &line.inlines {
                if let MarkdownInline::Image { src, .. } = inline {
                    urls.insert(src.clone());
                }
            }
        }

        let mut out = HashMap::with_capacity(urls.len());
        for src in urls {
            let key = self.resolve_single_image_key(&src).await;
            out.insert(src, key);
        }
        out
    }

    async fn resolve_single_image_key(&self, src: &str) -> Option<String> {
        if self.app_credentials.is_none() {
            return None;
        }

        let loaded = match self.load_image(src).await {
            Ok(loaded) => loaded,
            Err(err) => {
                tracing::warn!(image_src = %src, error = %err, "feishu image load failed");
                return None;
            }
        };

        match self.upload_image(loaded).await {
            Ok(image_key) => Some(image_key),
            Err(err) => {
                tracing::warn!(image_src = %src, error = %err, "feishu image upload failed");
                None
            }
        }
    }

    async fn load_image(&self, src: &str) -> crate::Result<LoadedImage> {
        if src.starts_with("https://") {
            return self.load_remote_image(src).await;
        }

        if src.contains("://") {
            return Err(anyhow::anyhow!("unsupported image url scheme").into());
        }

        let bytes = std::fs::read(src).map_err(|err| anyhow::anyhow!("read image file: {err}"))?;
        if bytes.is_empty() {
            return Err(anyhow::anyhow!("image file is empty").into());
        }
        if bytes.len() > self.image_upload_max_bytes {
            return Err(anyhow::anyhow!("image file too large for upload").into());
        }

        let path = Path::new(src);
        let file_name = path
            .file_name()
            .and_then(|v| v.to_str())
            .filter(|v| !v.is_empty())
            .unwrap_or("image")
            .to_string();

        let content_type = guess_image_mime(path.extension().and_then(|v| v.to_str()));

        Ok(LoadedImage {
            bytes,
            file_name,
            content_type,
        })
    }

    async fn load_remote_image(&self, src: &str) -> crate::Result<LoadedImage> {
        let url = parse_and_validate_https_url_basic(src)?;
        let client =
            select_http_client(&self.client, self.timeout, &url, self.enforce_public_ip).await?;

        let resp = send_reqwest(client.get(url.clone()), "feishu image download").await?;
        let status = resp.status();
        if !status.is_success() {
            let body = match read_text_body_limited(resp, DEFAULT_MAX_RESPONSE_BODY_BYTES).await {
                Ok(body) => body,
                Err(err) => {
                    return Err(anyhow::anyhow!(
                        "feishu image download http error: {status} (failed to read response body: {err})"
                    )
                    .into());
                }
            };
            let summary = truncate_chars(body.trim(), 200);
            if summary.is_empty() {
                return Err(anyhow::anyhow!(
                    "feishu image download http error: {status} (response body omitted)"
                )
                .into());
            }
            return Err(anyhow::anyhow!(
                "feishu image download http error: {status}, response={summary}"
            )
            .into());
        }

        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .map(str::trim)
            .filter(|v| v.starts_with("image/"))
            .map(ToString::to_string)
            .unwrap_or_else(|| {
                guess_image_mime(Path::new(url.path()).extension().and_then(|v| v.to_str()))
            });

        let bytes = read_bytes_body_limited(resp, self.image_upload_max_bytes).await?;
        if bytes.is_empty() {
            return Err(anyhow::anyhow!("downloaded image is empty").into());
        }

        let file_name = Path::new(url.path())
            .file_name()
            .and_then(|v| v.to_str())
            .filter(|v| !v.is_empty())
            .unwrap_or("image")
            .to_string();

        Ok(LoadedImage {
            bytes,
            file_name,
            content_type,
        })
    }

    async fn upload_image(&self, image: LoadedImage) -> crate::Result<String> {
        let access_token = self.ensure_tenant_access_token().await?;
        let mut upload_url = self.webhook_url.clone();
        upload_url.set_path("/open-apis/im/v1/images");
        upload_url.set_query(None);

        let client = select_http_client(
            &self.client,
            self.timeout,
            &upload_url,
            self.enforce_public_ip,
        )
        .await?;

        let part = reqwest::multipart::Part::bytes(image.bytes)
            .file_name(image.file_name)
            .mime_str(&image.content_type)
            .map_err(|err| anyhow::anyhow!("set image part mime: {err}"))?;
        let form = reqwest::multipart::Form::new()
            .text("image_type", "message")
            .part("image", part);

        let resp = send_reqwest(
            client
                .post(upload_url)
                .bearer_auth(access_token)
                .multipart(form),
            "feishu image upload",
        )
        .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = match read_text_body_limited(resp, DEFAULT_MAX_RESPONSE_BODY_BYTES).await {
                Ok(body) => body,
                Err(err) => {
                    return Err(anyhow::anyhow!(
                        "feishu image upload http error: {status} (failed to read response body: {err})"
                    )
                    .into());
                }
            };
            let summary = truncate_chars(body.trim(), 200);
            if summary.is_empty() {
                return Err(anyhow::anyhow!(
                    "feishu image upload http error: {status} (response body omitted)"
                )
                .into());
            }
            return Err(anyhow::anyhow!(
                "feishu image upload http error: {status}, response={summary}"
            )
            .into());
        }

        let body = read_json_body_limited(resp, DEFAULT_MAX_RESPONSE_BODY_BYTES).await?;
        let code = body["code"].as_i64().unwrap_or(-1);
        if code != 0 {
            return Err(anyhow::anyhow!("feishu image upload api error: code={code}").into());
        }

        let image_key = body["data"]["image_key"]
            .as_str()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| anyhow::anyhow!("feishu image upload api error: missing image_key"))?;

        Ok(image_key.to_string())
    }

    async fn ensure_tenant_access_token(&self) -> crate::Result<String> {
        let Some(credentials) = self.app_credentials.as_ref() else {
            return Err(anyhow::anyhow!(
                "feishu app credentials are required for markdown image upload"
            )
            .into());
        };

        {
            let guard = self.tenant_access_token.lock().await;
            if let Some(cached) = guard.as_ref() {
                if cached.expires_at > Instant::now() {
                    return Ok(cached.token.clone());
                }
            }
        }

        let mut token_url = self.webhook_url.clone();
        token_url.set_path("/open-apis/auth/v3/tenant_access_token/internal");
        token_url.set_query(None);

        let client = select_http_client(
            &self.client,
            self.timeout,
            &token_url,
            self.enforce_public_ip,
        )
        .await?;

        let payload = serde_json::json!({
            "app_id": credentials.app_id,
            "app_secret": credentials.app_secret,
        });

        let resp = send_reqwest(
            client.post(token_url).json(&payload),
            "feishu tenant access token",
        )
        .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = match read_text_body_limited(resp, DEFAULT_MAX_RESPONSE_BODY_BYTES).await {
                Ok(body) => body,
                Err(err) => {
                    return Err(anyhow::anyhow!(
                        "feishu tenant access token http error: {status} (failed to read response body: {err})"
                    )
                    .into());
                }
            };
            let summary = truncate_chars(body.trim(), 200);
            if summary.is_empty() {
                return Err(anyhow::anyhow!(
                    "feishu tenant access token http error: {status} (response body omitted)"
                )
                .into());
            }
            return Err(anyhow::anyhow!(
                "feishu tenant access token http error: {status}, response={summary}"
            )
            .into());
        }

        let body = read_json_body_limited(resp, DEFAULT_MAX_RESPONSE_BODY_BYTES).await?;
        let code = body["code"].as_i64().unwrap_or(-1);
        if code != 0 {
            return Err(
                anyhow::anyhow!("feishu tenant access token api error: code={code}").into(),
            );
        }

        let token = body["tenant_access_token"]
            .as_str()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| anyhow::anyhow!("feishu tenant access token api error: missing token"))?
            .to_string();

        let expires_in = body["expire"]
            .as_i64()
            .or_else(|| body["expires_in"].as_i64())
            .unwrap_or(7200)
            .max(120) as u64;
        let expires_at = Instant::now() + Duration::from_secs(expires_in.saturating_sub(60));

        let mut guard = self.tenant_access_token.lock().await;
        *guard = Some(AccessTokenCache {
            token: token.clone(),
            expires_at,
        });
        Ok(token)
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

fn read_bytes_body_limited(
    resp: reqwest::Response,
    max_bytes: usize,
) -> BoxFuture<'static, crate::Result<Vec<u8>>> {
    Box::pin(async move {
        let mut stream = resp.bytes_stream();
        let mut out = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|err| anyhow::anyhow!("read response bytes: {err}"))?;
            if out.len().saturating_add(chunk.len()) > max_bytes {
                return Err(anyhow::anyhow!("response body exceeds byte limit").into());
            }
            out.extend_from_slice(&chunk);
        }
        Ok(out)
    })
}

fn guess_image_mime(ext: Option<&str>) -> String {
    match ext
        .map(|v| v.trim().to_ascii_lowercase())
        .as_deref()
        .unwrap_or("")
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "heic" => "image/heic",
        _ => "application/octet-stream",
    }
    .to_string()
}

fn normalize_secret(secret: impl Into<String>) -> crate::Result<String> {
    let secret = secret.into();
    let secret = secret.trim();
    if secret.is_empty() {
        return Err(anyhow::anyhow!("feishu secret must not be empty").into());
    }
    Ok(secret.to_string())
}

fn normalize_optional_trimmed(value: Option<String>, field: &str) -> crate::Result<Option<String>> {
    match value {
        Some(value) => {
            let value = value.trim();
            if value.is_empty() {
                return Err(anyhow::anyhow!("feishu {field} must not be empty").into());
            }
            Ok(Some(value.to_string()))
        }
        None => Ok(None),
    }
}

fn normalize_app_credentials(
    app_id: Option<String>,
    app_secret: Option<String>,
) -> crate::Result<Option<FeishuAppCredentials>> {
    let app_id = normalize_optional_trimmed(app_id, "app_id")?;
    let app_secret = normalize_optional_trimmed(app_secret, "app_secret")?;

    match (app_id, app_secret) {
        (None, None) => Ok(None),
        (Some(app_id), Some(app_secret)) => Ok(Some(FeishuAppCredentials { app_id, app_secret })),
        _ => Err(
            anyhow::anyhow!("feishu app credentials must include both app_id and app_secret")
                .into(),
        ),
    }
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

            let payload = self
                .build_payload(event, timestamp.as_deref(), sign.as_deref())
                .await?;

            let resp = send_reqwest(
                client.post(self.webhook_url.as_str()).json(&payload),
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
    fn builds_expected_text_payload() {
        let event = Event::new("turn_completed", crate::Severity::Success, "done")
            .with_body("ok")
            .with_tag("thread_id", "t1");

        let payload = FeishuWebhookSink::build_text_payload(&event, FEISHU_MAX_CHARS, None, None);
        assert_eq!(payload["msg_type"].as_str().unwrap_or(""), "text");
        let text = payload["content"]["text"].as_str().unwrap_or("");
        assert!(text.contains("done"));
        assert!(text.contains("ok"));
        assert!(text.contains("thread_id=t1"));
    }

    #[test]
    fn builds_post_payload_for_markdown_body() {
        let event = Event::new("turn_completed", crate::Severity::Success, "done")
            .with_body("hello [lark](https://open.feishu.cn)\n\n![img](https://example.com/a.png)")
            .with_tag("thread_id", "t1");

        let sink = FeishuWebhookSink::new(FeishuWebhookConfig::new(
            "https://open.feishu.cn/open-apis/bot/v2/hook/x",
        ))
        .expect("build sink");

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build runtime");
        let payload = rt
            .block_on(sink.build_payload(&event, None, None))
            .expect("build payload");
        assert_eq!(payload["msg_type"].as_str().unwrap_or(""), "post");

        let content = payload["content"]["post"]["zh_cn"]["content"]
            .as_array()
            .expect("array content");
        assert!(!content.is_empty());

        let text_payload = payload.to_string();
        assert!(text_payload.contains("\"tag\":\"a\""), "{text_payload}");
        assert!(text_payload.contains("thread_id=t1"), "{text_payload}");
        assert!(text_payload.contains("[image:img]"), "{text_payload}");
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
        let payload = FeishuWebhookSink::build_text_payload(
            &event,
            FEISHU_MAX_CHARS,
            Some("123"),
            Some("sig"),
        );
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
        let payload = FeishuWebhookSink::build_text_payload(&event, 10, None, None);
        let text = payload["content"]["text"].as_str().unwrap_or("");
        assert!(text.chars().count() <= 10, "{text}");
        assert!(text.ends_with("..."), "{text}");
    }

    #[test]
    fn normalizes_app_credentials() {
        let cfg = FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/x")
            .with_app_credentials("  app_id  ", "  app_secret  ");
        let sink = FeishuWebhookSink::new(cfg).expect("build sink");
        let creds = sink.app_credentials.expect("credentials");
        assert_eq!(creds.app_id, "app_id");
        assert_eq!(creds.app_secret, "app_secret");
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
