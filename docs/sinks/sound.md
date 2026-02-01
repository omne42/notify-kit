# SoundSink

`SoundSink` 提供两种模式：

1) 默认：向 stderr 写入终端 bell（`\u{0007}`）
2) 自定义：执行外部命令播放提示音

## 终端 bell（默认）

```rust
use notify_kit::{SoundConfig, SoundSink};

let sink = SoundSink::new(SoundConfig { command_argv: None });
```

不同 `Severity` 会对应不同次数的 bell（用于区分提示强度）。

## 外部命令

```rust
use notify_kit::{SoundConfig, SoundSink};

let sink = SoundSink::new(SoundConfig {
    command_argv: Some(vec!["afplay".into(), "/System/Library/Sounds/Glass.aiff".into()]),
});
```

### 多平台提示

外部命令完全由你决定，本库只负责 spawn：

- macOS：`afplay <path>`
- Linux（示例）：`paplay <path>` / `aplay <path>`
- Windows：可用任意你习惯的播放器/脚本（例如 powershell）

建议把命令作为**本机配置**管理，而不是写死在代码里。

注意：

- 外部命令会被 spawn，并在后台线程中 wait 回收进程（避免僵尸进程累积）。
- `command_argv` 属于**本机受信任配置**；不要把不可信输入拼到 argv 里。
