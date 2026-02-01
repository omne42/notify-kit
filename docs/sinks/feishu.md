# FeishuWebhookSink

`FeishuWebhookSink` 通过飞书群机器人 webhook 发送 **text** 消息。

## 构造

```rust
use notify_kit::{FeishuWebhookConfig, FeishuWebhookSink};

let cfg = FeishuWebhookConfig::new("https://open.feishu.cn/open-apis/bot/v2/hook/xxx");
let sink = FeishuWebhookSink::new(cfg)?;
```

## 超时

`FeishuWebhookConfig` 自带一个 HTTP timeout（默认 `2s`）。此外，`Hub` 也会对每个 sink 做兜底超时：

- 建议：`HubConfig.per_sink_timeout` ≥ `FeishuWebhookConfig.timeout`
- 如果你把 `Hub` 的超时设得更小，那么即使 HTTP 还没超时，也会被 `Hub` 先中断（drop future）

## 安全约束（重要）

为降低 SSRF/凭据泄露风险，本库会对 webhook URL 做限制：

- 必须是 `https`
- 不允许携带 username/password
- host 仅允许：
  - `open.feishu.cn`
  - `open.larksuite.com`
- 不允许 `localhost` 或 IP
- 如显式指定端口，仅允许 `443`
- 禁用重定向（redirect）
- `Debug` 输出默认脱敏（不会泄露完整 webhook URL）

## 输出格式

文本内容由以下部分组成（按顺序）：

1) `title`
2) `body`（如果存在且非空）
3) 每个 tag：`key=value`（逐行）

## 错误信息（刻意保持“低敏感”）

为避免泄露敏感信息：

- 请求失败时的错误会被简化为类别（例如 `timeout/connect/request/...`）
- 非 2xx 的响应不会包含 response body（避免 body 中包含内部信息）
