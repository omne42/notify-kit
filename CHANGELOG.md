# Changelog

All notable changes to this project will be documented in this file.

The format is based on *Keep a Changelog*, and this project adheres to *Semantic Versioning*.

## [Unreleased]

### Added

### Changed

### Fixed

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
