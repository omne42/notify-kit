# Bots

本目录用于存放“交互式 bot / 集成程序”（例如 OpenCode 的 Slack bot）。

> `notify-kit` 的核心仍然是 Rust 通知库（`Hub` + `sinks`）。这里的 bot 只是“上层集成示例”，用于把某个系统的事件/消息桥接到通知/会话系统。

## opencode-slack

OpenCode 风格的 Slack bot（Socket Mode）：把 Slack thread 映射为会话，并在 thread 里输出会话分享链接与工具执行更新。

见：`bots/opencode-slack/README.md`

## opencode-feishu

OpenCode 风格的飞书（Lark/Feishu）事件订阅 bot：把群聊映射为会话，并在群里输出会话分享链接与工具执行更新。

见：`bots/opencode-feishu/README.md`

## opencode-dingtalk-stream

OpenCode 风格的钉钉 Stream Mode bot：把会话（sessionWebhook）映射为会话，并在群里输出会话分享链接与工具执行更新。

见：`bots/opencode-dingtalk-stream/README.md`
