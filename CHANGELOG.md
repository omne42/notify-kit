# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Added
- `Hub::try_notify`：当缺少 Tokio runtime 时返回错误（避免静默丢通知）。
- `Hub::send(event).await`：提供可观测的发送结果（等待所有 sinks 完成/超时）。

### Changed
- `FeishuWebhookSink`：限制 webhook URL（`https` + host allowlist），禁用重定向，错误信息不再包含响应 body。
- Docs: add GitBook-style documentation under `docs/` and link from README.

### Fixed
- `SoundSink`：外部命令会被回收（避免僵尸进程累积）。
- `FeishuWebhookConfig`/`FeishuWebhookSink`：`Debug` 输出不再泄露完整 webhook URL。

## [0.1.0] - 2026-01-31

### Added
- `notify-kit` crate：提供 `Hub` + `Sink` 抽象。
- `sound` sink：终端 bell / 自定义播放命令。
- `feishu` sink：飞书 webhook（text 消息）。
- `HubConfig`：支持可选 kind allow-list 与 per-sink timeout。

### Changed
- `Event.kind` 改为字符串（通用事件类型，不绑定具体业务域）。
- 移除库内置的 `CODE_PM_NOTIFY_*` 环境变量解析（交由上层 integration 负责）。

### Fixed
