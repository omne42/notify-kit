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
- `notify-kit/sound-command`：允许 `SoundSink` 执行外部命令播放提示音（默认关闭，未启用时回退为终端 bell）。
- `bots/opencode-slack`：OpenCode 风格的 Slack Socket Mode bot 示例（thread → session）。
- `bots/opencode-feishu`：OpenCode 风格的飞书 bot 示例（chat → session）。
- `bots/opencode-dingtalk-stream`：OpenCode 风格的钉钉 Stream Mode bot 示例（sessionWebhook → session）。
- `bots/opencode-github-action`：OpenCode 风格的 GitHub Actions 评论 bot 示例（Issue/PR comment → session）。
- `bots/opencode-wecom`：OpenCode 风格的企业微信（WeCom）回调 bot 示例（消息回调 → session）。
- `bots/opencode-discord`：OpenCode 风格的 Discord bot 示例（channel/thread → session）。
- `bots/opencode-telegram`：OpenCode 风格的 Telegram bot 示例（chat → session，long polling）。
- `bots/_shared/session_store`：支持设置根目录（`rootDir`；bots 可用 `OPENCODE_SESSION_STORE_ROOT`）以限制 session store 文件路径。
- `bots/_shared/opencode`：抽取 bots 共享逻辑（`assertEnv`/response 文本拼装/tool update 解析）。
- `bots/_shared/log`：提供 `ignoreError`（best-effort 忽略错误）与 `OPENCODE_BOT_VERBOSE` 可选日志输出。
- `bots/_shared/bootstrap`：抽取 bots 通用初始化（limiter + session store）。
- CI: GitHub Actions workflow（`./.github/workflows/ci.yml`）。
- Docs: 刷新 `docs/README.md`/`docs/concepts.md` 的内置 sinks 列表；`.gitignore` 忽略 `node_modules/`。
- Docs: 新增 mdBook 本地预览（含搜索）（`docs/book.toml` + `./scripts/docs.sh`）。
- Docs: 新增 `llms.txt` 聚合文档（`./scripts/build-llms-txt.sh` 生成）。
- Docs: 新增 `docs/llms.md` 与 `docs/changelog.md`。
- Docs: 新增 `docs/bots.md`，集中说明 OpenCode 风格 bot 示例。
- Docs: 新增 `docs/examples.md`（Examples / Recipes）与 sinks 速览矩阵。
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
- `HubConfig`：默认 `per_sink_timeout` 从 `2s` 调整为 `5s`，避免 HTTP sinks 默认超时与 DNS 预检叠加导致的误超时。
- `Hub`：`notify/try_notify` 日志路径不再为 `Event.kind` 进行多余的 `String` 克隆；过载丢弃路径也避免提前分配 `Arc<Event>`。
- `Hub`：聚合 sink 错误消息时改为直接写入 `String`（减少临时 `format!` 分配）。
- `FeishuWebhookSink`：限制 webhook URL（`https` + host allowlist），禁用重定向，错误信息不再包含响应 body。
- All built-in webhook sinks: 校验 URL path 前缀；消息构造改为“有上限”的截断与 tag cap；解析 JSON response 时限制最大读取大小（默认 `16KiB`）。
- Webhook/API sinks: 默认启用 DNS 公网 IP 校验（发送前执行，可关闭）。
- `GenericWebhookSink`：关闭 DNS 公网 IP 校验时，要求同时配置 `allowed_hosts`（减少 SSRF 风险）。
- `bots/_shared/limiter`：队列出队从 `Array.shift()` 改为游标 + 周期压缩，避免高积压时的 O(n) 复制开销。
- `sinks/text`：字段截断在“未发生截断”的常见路径改为复用借用字符串，减少临时 `String` 分配与拷贝。
- `bots/opencode-slack` / `bots/opencode-feishu` / `bots/opencode-wecom` / `bots/opencode-dingtalk-stream`：tool update 路径改为 `sessionId` 反向索引查找，避免每次事件线性扫描全部会话。
- Docs: 统一为 mdBook 文档（`./scripts/docs.sh` 本地预览/测试）。
- Docs: 文档 Rust 代码示例统一标注为 `edition2024`。
- Docs: mdBook 安装命令统一使用 `cargo install mdbook --locked`（更可复现）。
- Dev: 在提交门禁中增加 bot 示例的 Node.js 语法校验（不要求安装依赖）。
- Docs: 重构 `docs/SUMMARY.md` 的信息架构（Overview / Getting Started / Guides / Reference / Sinks）。
- Docs: `./scripts/docs.sh` 允许透传 mdBook 参数（便于容器/远程预览）。
- Docs: `llms.txt` 生成时会剔除 mdBook 的隐藏行（`# ...`），减少噪音。
- Dev: `githooks/pre-commit` 新增严格门禁（`scripts/pre-commit-check.sh`），提交前执行 clippy（`-D warnings`）与生产目标关键 lint（`unwrap/expect`、`let _ =` 忽略 must_use、冗余 clone）。

### Fixed
- `SoundSink`：外部命令会被回收（避免僵尸进程累积）。
- `SoundSink`：等待子进程改为使用 Tokio 的 blocking pool（避免线程创建失败导致 panic）。
- `SoundSink`：拒绝空 program 的错误配置。
- `FeishuWebhookConfig`/`FeishuWebhookSink`：`Debug` 输出不再泄露完整 webhook URL。
- `SoundSink`：调整测试模块位置以通过 clippy（`items_after_test_module`）。
- `SoundSink`：移除无用 `mut` 以通过 clippy（`unused_mut`）。
- `SlackWebhookSink`：2xx 响应时会读取并校验响应 body（避免 200 + 错误文本被误判为成功）。
- `dingtalk` / `wecom` / `feishu` sinks：2xx 响应但 body 非 JSON/读取失败时不再误判为成功（解析失败会返回错误）。
- `BarkSink`：补充 API 级错误判断（当响应为 JSON 且包含 `code` 时），并在非 2xx 时附带截断后的响应摘要。
- `DiscordWebhookSink` / `GenericWebhookSink`：非 2xx 时附带截断后的响应摘要，便于定位问题。
- `serverchan` sink：错误信息不再回显第三方返回的 message（保持低敏感）。
- `Hub`：sink task panic 时错误聚合现在会保留 sink 名称（便于定位）。
- `Hub`：聚合 sink 结果时不再为 sink 名称分配 `String`（减少堆分配）。
- Webhook/API sinks: 修复 `enforce_public_ip` 打开时未实际使用 pinned client 的问题。
- Webhook/API sinks: 公网 IP 判定现在会正确处理 IPv4-mapped IPv6（例如 `::ffff:127.0.0.1`），避免绕过 SSRF 防护。
- Webhook/API sinks: 公网 IP 判定现在会识别 NAT64 well-known prefix `64:ff9b::/96`，按嵌入的 IPv4 再判定（兼容 DNS64 且避免绕过 SSRF 防护）。
- Webhook/API sinks: 公网 IP 判定现在会识别 6to4 `2002::/16`，按嵌入的 IPv4 再判定（避免绕过 SSRF 防护）。
- Webhook/API sinks: IPv6 公网 IP 判定现在会拒绝 site-local `fec0::/10`（例如 `fec0::1`）。
- Webhook/API sinks: IPv4 公网 IP 判定补齐更多 RFC6890 特殊用途网段（例如 `192.0.0.0/24`、`192.88.99.0/24`）。
- Webhook/API sinks: `dns lookup timeout` 错误现在会注明 DNS 超时上限为 `2s`（`min(timeout, 2s)`）。
- Webhook/API sinks: `dns lookup failed` 错误现在会保留底层错误信息（便于排障）。
- Webhook/API sinks: DNS 公网 IP 预检的超时预算现在按“总时限”生效（信号量等待 + DNS 查询共享同一 budget），避免高并发下超时被阶段性叠加放大。
- Webhook/API sinks: `decode json failed` 错误现在会保留底层解析错误信息（便于排障）。
- Webhook/API sinks: URL path 前缀校验改为“段边界匹配”（例如 `/send` 不再匹配 `/sendMessage`），减少误放行。
- Webhook/API sinks: DNS 解析结果去重改为 `HashSet`（避免 O(n²) 扫描）。
- API: 公共签名统一使用 `notify_kit::Result` / `notify_kit::Error`；其中 `notify_kit::Error` 现在是对 `anyhow::Error` 的薄封装（避免在公共 API 中暴露 `anyhow` 类型）。
- Webhook/API sinks: 收敛严格模式下的同步 DNS 预检实现，移除 per-host inflight/cache（仍保持有界并发与超时）。
- Webhook/API sinks: 严格模式同步 DNS 预检在 thread spawn 失败时会保留底层错误信息（便于排障）。
- `FeishuWebhookSink::new_strict` / `new_with_secret_strict`：严格模式下禁止关闭公网 IP 校验。
- `bots/opencode-feishu`：修正 Feishu SDK 的 ESM 导入与事件名（`im.message.receive_v1`），并启用 callback challenge 自动处理。
- `bots/opencode-github-action`：修正示例安装命令为 `npm install`（仓库未提供 lockfile，避免 `npm ci` 失败）。
- `bots/opencode-github-action`：当 OpenCode prompt 返回 error 时会 fail（避免发布空响应评论）。
- `bots/opencode-wecom`：回调解密后校验 receiver（corp id），并加强 PKCS7 padding 校验。
- `bots/opencode-wecom`：校验解密后的 `msgLen` 边界，避免越界读取。
- `bots/opencode-dingtalk-stream`：校验 `sessionWebhook` 为 https 且 host 属于钉钉域名（降低 SSRF 风险）。
- `bots/opencode-wecom`：增加 timestamp 时间窗与 nonce 去重（降低重放风险）。
- `bots/opencode-wecom`：签名比较使用 timingSafeEqual，并对 replay cache 增加容量上限（避免 DoS / 内存增长）。
- `bots/opencode-wecom`：SHA1 hex 解析增加长度快速判断并用手写 hex 校验替代正则（避免超长输入触发正则扫描）。
- Webhook/API sinks: DNS 公网 IP 校验增加超时与并发限制，避免阻塞/线程池耗尽，并对 pinned client 做短 TTL 缓存以减少重复解析。
- Webhook/API sinks: DNS 公网 IP 校验：timeout 后将 permit 生命周期绑定到实际解析任务（防止持续 timeout 时产生无界 blocking 任务/线程），并在 timeout 路径上清理 inflight 条目避免 map 增长，同时为失败/超时结果增加短 TTL 负缓存。
- Webhook/API sinks: `pinned client` 构建锁在失败路径下会及时移除对应 `Weak` 条目，避免失败 host 累积导致静态锁表增长。
- `SlackWebhookSink` / `DiscordWebhookSink` / `GenericWebhookSink` / `BarkSink`：读取响应 body 失败时仍保留 HTTP status 错误上下文。
- `BarkSink`：当响应看起来像 JSON 时，即使 Content-Type 缺失/错误也会尝试解析。
- `bots/opencode-telegram`：支持 `OPENCODE_SESSION_STORE_PATH` 以持久化 chat → session 映射（可选）。
- `bots/_shared/session_store`：`rootDir` 校验加强（防 symlink 逃逸；flush 前二次校验 realpath）。
- `bots/_shared/session_store`：flush 失败在非 verbose 下也会输出一次性 warning（避免静默失败）。
- `bots/_shared/session_store`：退出 hook 支持多个 store 实例（避免只 flush 第一个）。
- `bots/_shared/session_store`：`close()` 会取消 debounce 并触发一次 flush（返回 Promise），同时解除 exit hook 注册（避免短生命周期 store 累积 flush 回调）。
- `bots/_shared/opencode`：`runEventSubscriptionLoop` 在并发未打满且事件流暂时阻塞时，也会及时消费 `onEvent` 失败并触发重连（避免卡死在单次订阅）。
- `FeishuWebhookSink`：严格模式下的构造期 DNS 公网 IP 校验增加超时 + 并发限制（避免 DNS 卡死导致初始化阻塞/线程堆积）。
- `FeishuWebhookSink`：严格模式下的构造期 DNS 公网 IP 校验增加 inflight 去重 + TTL 缓存，并对 pinned client cache 增加容量上限（避免重复/无界增长）。
- `bots/_shared/log`：verbose 模式输出错误 stack（更易排障）。
- Docs: 修复 `./scripts/docs.sh test` 偶发的重复 rmeta 导致的 snippet 编译失败。

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
