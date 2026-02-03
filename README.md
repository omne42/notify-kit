# notify-kit

一个轻量的通知 Hub（Rust），用于把任意事件推送到多个通知渠道（sinks）。

当前实现：

- `sound`：终端 bell（默认）或自定义播放命令
- `feishu`：飞书群机器人 webhook（text 消息，可选签名）
- `github`：GitHub Issues/PR 评论（text）
- `slack`：Slack Incoming Webhook（text 消息）
- `discord`：Discord webhook（text 消息）
- `telegram`：Telegram Bot API（sendMessage）
- `serverchan`：Server酱（ServerChan）推送（text）
- `pushplus`：PushPlus 推送（text）
- `bark`：Bark 推送（text）
- `webhook`：通用 JSON webhook（`{text: ...}` 或自定义字段）
- `dingtalk`：钉钉群机器人 webhook（text 消息，可选签名）
- `wecom`：企业微信群机器人 webhook（text 消息）

设计目标：

- 可扩展：后续追加 email/discord/slack/tgbot/桌宠…只需要新增 sink
- 不阻塞：通知发送失败/超时不会卡住主流程（每个 sink 有超时）

## 文档

- GitBook：`docs/README.md`（目录：`docs/SUMMARY.md`）
- 本地预览（含搜索）：`./scripts/docs.sh serve`（需要先 `cargo install mdbook`）
- Rustdoc：`cargo doc -p notify-kit --open`
- LLM 友好入口：`llms.txt`（由 `./scripts/build-llms-txt.sh` 生成）

## Bots（上层集成示例）

本仓库的核心是 Rust 通知库（`Hub` + `sinks`）。另外也提供少量“上层 bot/集成示例”：

- `bots/`（见 `bots/README.md`）

## 用法

`Hub::notify` 是 fire-and-forget：在 **Tokio runtime** 中 spawn 后台任务并立即返回。

- 如果当前没有 Tokio runtime：`notify` 会丢弃通知并 `tracing::warn!`；可用 `Hub::try_notify` 检测。
- 如果需要可观测结果：用 `Hub::send(event).await`（会等待所有 sinks 完成/超时）。

最小示例（需要在 Tokio runtime 中调用）：

```rust
use std::sync::Arc;

use notify_kit::{Event, Hub, HubConfig, Severity, SoundConfig, SoundSink};

let hub = Hub::new(
    HubConfig::default(),
    vec![Arc::new(SoundSink::new(SoundConfig { command_argv: None }))],
);

hub.notify(Event::new("turn_completed", Severity::Success, "done"));
```

## 安全提示

- `SoundConfig.command_argv` 会执行外部命令；应视为 **受信任/本机配置**。
- `FeishuWebhookSink` 会校验 webhook URL：仅允许 `https` + `open.feishu.cn` / `open.larksuite.com`，且不会在 `Debug`/错误信息中输出完整 URL。

## 配置（环境变量）

本库不规定环境变量协议；配置应由上层应用负责（比如 integration 层解析 env，然后构造 sinks + Hub）。

## 与 omne-agent 集成

`omne-agent` 仓库（目录名为 `omne-agent/`）内的 `omne-agent-app-server` notify integration 负责解析 `OMNE_AGENT_NOTIFY_*` 并构造 Hub；通过 feature `notify` 集成（默认关闭）。示例：

```bash
cd ../omne-agent

export OMNE_AGENT_NOTIFY_SOUND=1
# export OMNE_AGENT_NOTIFY_FEISHU_WEBHOOK_URL="..."
# export OMNE_AGENT_NOTIFY_EVENTS="turn_completed,approval_requested,message_received"

cargo run -p omne-agent-app-server --features notify
```

## 开发

离线检查：

```bash
CARGO_NET_OFFLINE=true ./scripts/gate.sh
```
