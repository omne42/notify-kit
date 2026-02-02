# 安全

## 外部命令执行（SoundSink）

`SoundConfig.command_argv` 会执行外部命令：

- 仅应由**本机受信任配置**提供
- 不要把不可信数据拼接到 argv（避免命令执行风险）

## Webhook（Feishu/Slack/Discord/钉钉/企微）

Webhook URL 属于敏感信息：

- 不要写入日志/错误信息/Debug 输出
- 使用配置系统安全存储（例如 secrets manager / 环境变量注入）
- 本库对 URL 做了 scheme/host/port/credentials 限制以降低 SSRF 风险

目前内置的 webhook sinks 允许的 host（精确匹配）：

- Feishu：`open.feishu.cn` / `open.larksuite.com`
- Slack：`hooks.slack.com`
- Discord：`discord.com` / `discordapp.com`
- 钉钉：`oapi.dingtalk.com`
- 企业微信：`qyapi.weixin.qq.com`
- Telegram：固定为 `api.telegram.org`

### 为什么要限制 host / 禁用重定向？

Webhook 发送本质是“服务端发起 HTTP 请求”。如果 URL 可被不可信输入影响，会引入 SSRF 风险。

本库的策略是：

- **允许的域名做 allow-list**（只放行官方 webhook 域名）
- **禁用重定向**（避免被 30x 绕过 allow-list）
- **校验 URL path 前缀**（避免误配到同域其它 endpoint）
- **错误信息保持低敏感**（不输出 body、不输出完整 URL）

### 可选：DNS 解析结果必须是公网 IP

如果你担心 DNS 污染 / 内网解析等风险，部分 sinks 提供可选开关：在构造 sink 时做一次 DNS 解析校验（解析到私网/loopback/link-local 会拒绝）。

注意：这是一个“更严格、更保守”的策略；在无网络/DNS 不可用时也可能导致构造失败。

## DoS / 噪音控制

为了避免异常大消息或事件洪泛导致内存/网络放大，本库内置 sinks 会对内容做截断与上限：

- 文本总长度：按 sink 的 `max_chars`（或内置默认）截断并追加 `...`
- tags 数量与 tag key/value 长度：超出会截断/忽略（避免极端情况下构建超大 payload）
- JSON response：只会读取有限大小（默认 `16KiB`），并且错误信息不会包含 response body

另外，`Hub::notify` 内部有一个固定的 inflight 限制；超过上限会丢弃并 `warn`（避免无界 spawn 造成 DoS）。

## 错误信息与敏感数据

实现自定义 sink 时，建议：

- 错误信息避免包含 token、完整 URL、用户隐私数据
- `Debug` 输出对敏感字段做脱敏

## Event 内容也是敏感数据

`Event.title/body/tags` 由上层业务提供，可能包含：

- 用户输入
- 仓库路径/机器信息
- 错误堆栈

在实现 sink 时，建议把“对外发送的内容”当作需要审计的出口：

- 限制最大长度
- 对高敏感字段做删减/脱敏
- 必要时引入 allow-list（只发部分 kind）
