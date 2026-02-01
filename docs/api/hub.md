# Hub

`Hub` 是通知中心：把一个 `Event` 广播到多个 sinks。

## 构造

```rust
use std::sync::Arc;
use notify_kit::{Hub, HubConfig, SoundConfig, SoundSink};

let hub = Hub::new(
    HubConfig::default(),
    vec![Arc::new(SoundSink::new(SoundConfig { command_argv: None }))],
);
```

## HubConfig

- `enabled_kinds: Option<BTreeSet<String>>`
  - `None`：不过滤
  - `Some(set)`：仅允许 set 内 kind
- `per_sink_timeout: Duration`
  - 默认 `2s`
  - 作为兜底，避免任何 sink 卡住调用方

一个更完整的配置示例：

```rust
use std::collections::BTreeSet;
use std::time::Duration;

use notify_kit::HubConfig;

let enabled_kinds = BTreeSet::from(["turn_completed".to_string(), "approval_requested".to_string()]);
let cfg = HubConfig {
    enabled_kinds: Some(enabled_kinds),
    per_sink_timeout: Duration::from_secs(5),
};
```

## 发送接口

- `notify(event)`: fire-and-forget；无 runtime 时会丢弃并记录 warning
- `try_notify(event)`: 同上，但缺少 runtime 时返回 `TryNotifyError::NoTokioRuntime`
- `send(event).await`: 等待所有 sinks 完成/超时；失败时聚合错误并返回

## 行为细节

- **kind 被禁用时是 no-op**：即使没有 Tokio runtime 也不会报错（直接返回）。
- **并发发送**：`send().await` 会并发调用所有 sinks。
- **每个 sink 单独超时**：由 `per_sink_timeout` 控制；超时会被视为该 sink 失败。
- **错误聚合**：当一个或多个 sinks 失败时，会返回一个聚合错误，内容类似：

```
one or more sinks failed:
- feishu: timeout after 2s
- sound: boom
```

