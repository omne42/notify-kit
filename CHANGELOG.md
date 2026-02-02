# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Added
- `Hub::try_notify`：当缺少 Tokio runtime 时返回错误（避免静默丢通知）。
- `Hub::send(event).await`：提供可观测的发送结果（等待所有 sinks 完成/超时）。
- `Hub::new_with_inflight_limit`：限制 `notify()` 的后台并发，超限会丢弃并 warning（背压/防 DoS）。
- `FeishuWebhookConfig`：新增 `max_chars`/`with_max_chars` 与 `enforce_public_ip`/`with_public_ip_check`。
- `GenericWebhookConfig::new_strict` / `GenericWebhookSink::new_strict`：提供更严格的 SSRF 防护（强制 host allow-list + path 前缀 + 公网 IP 校验）。
- `bots/opencode-slack`：OpenCode 风格的 Slack Socket Mode bot 示例（thread → session）。
- `bots/opencode-feishu`：OpenCode 风格的飞书 bot 示例（chat → session）。
- `bots/opencode-dingtalk-stream`：OpenCode 风格的钉钉 Stream Mode bot 示例（sessionWebhook → session）。
- Docs: 刷新 `docs/README.md`/`docs/concepts.md` 的内置 sinks 列表；`.gitignore` 忽略 `node_modules/`。
- New sinks:
  - `SlackWebhookSink`：Slack Incoming Webhook（text）。
  - `DiscordWebhookSink`：Discord webhook（text）。
  - `TelegramBotSink`：Telegram Bot API（sendMessage）。
  - `DingTalkWebhookSink`：钉钉群机器人 webhook（text，可选签名）。
  - `WeComWebhookSink`：企业微信群机器人 webhook（text）。
  - `GitHubCommentSink`：GitHub Issue/PR 评论（text）。
  - `ServerChanSink`：Server酱（ServerChan）推送（text）。
  - `PushPlusSink`：PushPlus 推送（text）。
  - `BarkSink`：Bark 推送（text）。
  - `GenericWebhookSink`：通用 JSON webhook（默认 `{text: ...}`）。
- `FeishuWebhookSink::new_with_secret`：支持飞书群机器人 webhook 签名（timestamp/sign）。
- `FeishuWebhookSink::new_strict` / `new_with_secret_strict`：在构造阶段额外做一次 DNS 公网 IP 校验。

### Changed
- `FeishuWebhookSink`：限制 webhook URL（`https` + host allowlist），禁用重定向，错误信息不再包含响应 body。
- All built-in webhook sinks: 校验 URL path 前缀；消息构造改为“有上限”的截断与 tag cap；解析 JSON response 时限制最大读取大小（默认 `16KiB`）。
- Webhook/API sinks: 默认启用 DNS 公网 IP 校验（发送前执行，可关闭）。
- Docs: add GitBook-style documentation under `docs/` and link from README.

### Fixed
- `SoundSink`：外部命令会被回收（避免僵尸进程累积）。
- `SoundSink`：拒绝空 program 的错误配置。
- `FeishuWebhookConfig`/`FeishuWebhookSink`：`Debug` 输出不再泄露完整 webhook URL。
- `SoundSink`：调整测试模块位置以通过 clippy（`items_after_test_module`）。
- `dingtalk` / `wecom` sink：2xx 响应但 body 非 JSON/读取失败时不再误判为失败（只在明确 errcode 非 0 时失败）。
- `serverchan` sink：错误信息不再回显第三方返回的 message（保持低敏感）。

## [0.1.0] - 2026-01-31

### Added
- `notify-kit` crate：提供 `Hub` + `Sink` 抽象。
- `sound` sink：终端 bell / 自定义播放命令。
- `feishu` sink：飞书 webhook（text 消息）。
- `HubConfig`：支持可选 kind allow-list 与 per-sink timeout。

### Changed
- `Event.kind` 改为字符串（通用事件类型，不绑定具体业务域）。
- 移除库内置的通知环境变量解析（交由上层 integration 负责）。

### Fixed
