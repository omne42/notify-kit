# notify-kit

一个轻量的通知 Hub（Rust），用于把任意事件推送到多个通知渠道（sinks）。

当前实现：

- `sound`：终端 bell（默认）或自定义播放命令
- `feishu`：飞书群机器人 webhook（text 消息）

设计目标：

- 可扩展：后续追加 email/discord/slack/tgbot/桌宠…只需要新增 sink
- 不阻塞：通知发送失败/超时不会卡住主流程（每个 sink 有超时）

## 配置（环境变量）

本库不规定环境变量协议；配置应由上层应用负责（比如 integration 层解析 env，然后构造 sinks + Hub）。

## 与 `codex_pm` 集成

`codex_pm` 内的 notify integration 负责解析 `CODE_PM_NOTIFY_*` 并构造 Hub；`pm-app-server` 通过 feature `notify` 集成（默认关闭）。示例：

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
