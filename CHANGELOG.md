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
- `FeishuWebhookSink`：限制 webhook URL（`https` + host allowlist），禁用重定向，错误信息不再包含响应 body。
- All built-in webhook sinks: 校验 URL path 前缀；消息构造改为“有上限”的截断与 tag cap；解析 JSON response 时限制最大读取大小（默认 `16KiB`）。
- Webhook/API sinks: 默认启用 DNS 公网 IP 校验（发送前执行，可关闭）。
- `GenericWebhookSink`：关闭 DNS 公网 IP 校验时，要求同时配置 `allowed_hosts`（减少 SSRF 风险）。
- Docs: 统一为 mdBook 文档（`./scripts/docs.sh` 本地预览/测试）。
- Dev: 在提交门禁中增加 bot 示例的 Node.js 语法校验（不要求安装依赖）。
- Docs: 重构 `docs/SUMMARY.md` 的信息架构（Overview / Getting Started / Guides / Reference / Sinks）。
- Docs: `./scripts/docs.sh` 允许透传 mdBook 参数（便于容器/远程预览）。
- Docs: `llms.txt` 生成时会剔除 mdBook 的隐藏行（`# ...`），减少噪音。

### Fixed
- `SoundSink`：外部命令会被回收（避免僵尸进程累积）。
- `SoundSink`：等待子进程改为使用 Tokio 的 blocking pool（避免线程创建失败导致 panic）。
- `SoundSink`：拒绝空 program 的错误配置。
- `FeishuWebhookConfig`/`FeishuWebhookSink`：`Debug` 输出不再泄露完整 webhook URL。
- `SoundSink`：调整测试模块位置以通过 clippy（`items_after_test_module`）。
- `SlackWebhookSink`：2xx 响应时会读取并校验响应 body（避免 200 + 错误文本被误判为成功）。
- `dingtalk` / `wecom` / `feishu` sinks：2xx 响应但 body 非 JSON/读取失败时不再误判为成功（解析失败会返回错误）。
- `BarkSink`：补充 API 级错误判断（当响应为 JSON 且包含 `code` 时），并在非 2xx 时附带截断后的响应摘要。
- `DiscordWebhookSink` / `GenericWebhookSink`：非 2xx 时附带截断后的响应摘要，便于定位问题。
- `serverchan` sink：错误信息不再回显第三方返回的 message（保持低敏感）。
- Webhook/API sinks: 修复 `enforce_public_ip` 打开时未实际使用 pinned client 的问题。
- `FeishuWebhookSink::new_strict` / `new_with_secret_strict`：严格模式下禁止关闭公网 IP 校验。
- `bots/opencode-feishu`：修正 Feishu SDK 的 ESM 导入与事件名（`im.message.receive_v1`），并启用 callback challenge 自动处理。
- `bots/opencode-github-action`：修正示例安装命令为 `npm install`（仓库未提供 lockfile，避免 `npm ci` 失败）。
- `bots/opencode-github-action`：当 OpenCode prompt 返回 error 时会 fail（避免发布空响应评论）。
- `bots/opencode-wecom`：回调解密后校验 receiver（corp id），并加强 PKCS7 padding 校验。
- `bots/opencode-wecom`：校验解密后的 `msgLen` 边界，避免越界读取。
- `bots/opencode-dingtalk-stream`：校验 `sessionWebhook` 为 https 且 host 属于钉钉域名（降低 SSRF 风险）。
- `bots/opencode-wecom`：增加 timestamp 时间窗与 nonce 去重（降低重放风险）。
- `bots/opencode-wecom`：签名比较使用 timingSafeEqual，并对 replay cache 增加容量上限（避免 DoS / 内存增长）。
- Webhook/API sinks: DNS 公网 IP 校验增加超时与并发限制，避免阻塞/线程池耗尽，并对 pinned client 做短 TTL 缓存以减少重复解析。
- Webhook/API sinks: DNS 公网 IP 校验在超时后不再长期占用并发 permit，并为失败结果增加短 TTL 负缓存（避免 DNS hang 导致持续退化）。
- `SlackWebhookSink` / `DiscordWebhookSink` / `GenericWebhookSink` / `BarkSink`：读取响应 body 失败时仍保留 HTTP status 错误上下文。
- `BarkSink`：当响应看起来像 JSON 时，即使 Content-Type 缺失/错误也会尝试解析。
- `bots/opencode-telegram`：支持 `OPENCODE_SESSION_STORE_PATH` 以持久化 chat → session 映射（可选）。
- `bots/_shared/session_store`：`rootDir` 校验加强（防 symlink 逃逸；flush 前二次校验 realpath）。
- `bots/_shared/session_store`：flush 失败在非 verbose 下也会输出一次性 warning（避免静默失败）。
- `bots/_shared/session_store`：退出 hook 支持多个 store 实例（避免只 flush 第一个）。
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
