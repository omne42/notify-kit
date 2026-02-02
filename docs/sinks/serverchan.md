# ServerChanSink

`ServerChanSink` 通过 Server酱（ServerChan）API 发送推送（纯文本）。

支持：

- Turbo（`SCT...`）：`sctapi.ftqq.com`
- SC3（`sctp{uid}t...`）：`{uid}.push.ft07.com`

## 构造

```rust
use notify_kit::{ServerChanConfig, ServerChanSink};

let cfg = ServerChanConfig::new("SCTxxxxxxxxxxxxxxxx");
let sink = ServerChanSink::new(cfg)?;
```

## 超时

`ServerChanConfig` 自带 HTTP timeout（默认 `2s`）。此外，`Hub` 也会对每个 sink 做兜底超时：

- 建议：`HubConfig.per_sink_timeout` ≥ `ServerChanConfig.timeout`

## 安全提示

- `send_key` 属于敏感信息：不要写入日志/错误信息/Debug 输出。
- 默认会做 DNS 公网 IP 校验（可通过 `with_public_ip_check(false)` 关闭）。
