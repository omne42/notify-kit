# codepm-notify

一个轻量的通知 Hub（Rust），用于把任务/对话的关键事件推送到多个通知渠道（sinks）。

当前实现：

- `sound`：终端 bell（默认）或自定义播放命令
- `feishu`：飞书群机器人 webhook（text 消息）

设计目标：

- 可扩展：后续追加 email/discord/slack/tgbot/桌宠…只需要新增 sink
- 不阻塞：通知发送失败/超时不会卡住主流程（每个 sink 有超时）

## 配置（环境变量）

- `CODE_PM_NOTIFY_SOUND=1`：启用声音
- `CODE_PM_NOTIFY_SOUND_CMD_JSON='["afplay","/System/Library/Sounds/Ping.aiff"]'`：可选，自定义播放命令 argv（JSON 数组）
- `CODE_PM_NOTIFY_FEISHU_WEBHOOK_URL=<url>`：启用飞书 webhook
- `CODE_PM_NOTIFY_EVENTS=turn_completed,approval_requested`：启用事件（默认启用 `turn_completed,approval_requested`）

可选事件：

- `turn_completed`
- `approval_requested`
- `message_received`

## 与 `codex_pm` 集成

`pm-app-server` 通过 feature `notify` 集成（默认关闭）。示例：

```bash
cd ../codex_pm

export CODE_PM_NOTIFY_SOUND=1
# export CODE_PM_NOTIFY_FEISHU_WEBHOOK_URL="..."
# export CODE_PM_NOTIFY_EVENTS="turn_completed,approval_requested,message_received"

cargo run -p pm-app-server --features notify
```

## 开发

离线检查：

```bash
CARGO_NET_OFFLINE=true ./scripts/gate.sh
```
