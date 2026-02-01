# 安全

## 外部命令执行（SoundSink）

`SoundConfig.command_argv` 会执行外部命令：

- 仅应由**本机受信任配置**提供
- 不要把不可信数据拼接到 argv（避免命令执行风险）

## Webhook（FeishuWebhookSink）

Webhook URL 属于敏感信息：

- 不要写入日志/错误信息/Debug 输出
- 使用配置系统安全存储（例如 secrets manager / 环境变量注入）
- 本库对 URL 做了 scheme/host/port/credentials 限制以降低 SSRF 风险

### 为什么要限制 host / 禁用重定向？

Webhook 发送本质是“服务端发起 HTTP 请求”。如果 URL 可被不可信输入影响，会引入 SSRF 风险。

本库的策略是：

- **允许的域名做 allow-list**（只放行官方 webhook 域名）
- **禁用重定向**（避免被 30x 绕过 allow-list）
- **错误信息保持低敏感**（不输出 body、不输出完整 URL）

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
