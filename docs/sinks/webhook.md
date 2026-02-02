# GenericWebhookSink

`GenericWebhookSink` 会向指定 URL POST 一个 JSON payload（默认 `{ "text": "..." }`）。

## 构造

```rust
use notify_kit::{GenericWebhookConfig, GenericWebhookSink};

let cfg = GenericWebhookConfig::new("https://example.com/webhook");
let sink = GenericWebhookSink::new(cfg)?;
```

可选：修改字段名、限制 URL path 前缀、限制允许的 host：

```rust
use notify_kit::{GenericWebhookConfig, GenericWebhookSink};

let cfg = GenericWebhookConfig::new("https://example.com/hooks/notify")
    .with_payload_field("content")
    .with_path_prefix("/hooks/")
    .with_allowed_hosts(vec!["example.com".to_string()]);
let sink = GenericWebhookSink::new(cfg)?;
```

## 安全提示

- 默认会做 DNS 公网 IP 校验（可通过 `with_public_ip_check(false)` 关闭）。
- 如果你使用 `allowed_hosts`，建议把它视为安全边界（不要从不可信输入构造）。
