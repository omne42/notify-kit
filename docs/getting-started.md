# 快速开始

本章给出最小可运行示例，并解释 `notify` 与 `send` 的差异。

## 安装

如果你通过 crates.io 使用：

```toml
[dependencies]
notify-kit = "0.1"
```

如果你通过 Git / monorepo 引用：

```toml
[dependencies]
notify-kit = { path = "../notify-kit/crates/notify-kit" }
```

> 以上版本与路径仅为示例；请按你的项目实际情况调整。

## 一个可运行的 `main.rs` 示例

`Hub::notify` 需要在 **Tokio runtime** 中调用（否则会丢弃并 `tracing::warn!`）。

```rust
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use notify_kit::{
    Event, Hub, HubConfig, Severity, Sink, SoundConfig, SoundSink, TryNotifyError,
};

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    // 你的应用应该初始化 tracing subscriber，这样才能看到 `notify-kit` 的 warn 日志。
    // 这里仅作为示例：你也可以使用自己项目里的 tracing 配置。
    //
    // tracing_subscriber = "0.3"
    tracing_subscriber::fmt::init();

    // 组合多个 sinks（示例只启用 sound）
    let sinks: Vec<Arc<dyn Sink>> = vec![Arc::new(SoundSink::new(SoundConfig { command_argv: None }))];

    // 可选：只允许一部分 kind
    let enabled_kinds: Option<BTreeSet<String>> =
        Some(BTreeSet::from(["turn_completed".to_string(), "approval_requested".to_string()]));

    let hub = Hub::new(
        HubConfig {
            enabled_kinds,
            per_sink_timeout: Duration::from_secs(2),
        },
        sinks,
    );

    // fire-and-forget（不关心结果）
    hub.notify(Event::new("turn_completed", Severity::Success, "done"));

    // 可观测结果（等待所有 sinks）
    hub.send(Event::new("turn_completed", Severity::Success, "done (awaited)"))
        .await?;

    // 如果你处在“不确定是否有 Tokio runtime”的代码路径中：
    match hub.try_notify(Event::new("turn_completed", Severity::Success, "done (try_notify)")) {
        Ok(()) => {}
        Err(TryNotifyError::NoTokioRuntime) => {
            // 这里不要 panic：notify 只是附加能力。
            // 你可以选择：记录日志、降级为 stdout、暂存到队列里、或忽略。
            tracing::debug!("no tokio runtime; notification skipped");
        }
    }

    Ok(())
}
```

## 我该用 `notify` 还是 `send`？

- `notify(event)`: fire-and-forget（spawn 后台任务并立即返回）
- `try_notify(event) -> Result<(), TryNotifyError>`: 同 `notify`，但可检测「缺少 Tokio runtime」
- `send(event).await -> anyhow::Result<()>`: 等待所有 sinks 完成/超时，并聚合错误信息

## 常见模式

### 同时启用多个 sinks

```rust
use std::sync::Arc;

use notify_kit::{FeishuWebhookConfig, FeishuWebhookSink, Sink, SoundConfig, SoundSink};

let mut sinks: Vec<Arc<dyn Sink>> = Vec::new();
// 本地提示音
sinks.push(Arc::new(SoundSink::new(SoundConfig { command_argv: None })));
// 飞书 webhook（注意：webhook URL 属于敏感信息，请用安全配置注入）
sinks.push(Arc::new(FeishuWebhookSink::new(FeishuWebhookConfig::new(
    "https://open.feishu.cn/open-apis/bot/v2/hook/xxx",
))?));
```

### 事件过滤（只发你关心的 kind）

```rust
use std::collections::BTreeSet;
use std::time::Duration;

use notify_kit::HubConfig;

let enabled_kinds = BTreeSet::from(["turn_completed".to_string(), "message_received".to_string()]);
let cfg = HubConfig {
    enabled_kinds: Some(enabled_kinds),
    per_sink_timeout: Duration::from_secs(2),
};
```

