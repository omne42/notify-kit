# 设计说明

## 目标

- **轻量**：作为基础库，依赖尽量少、集成成本低
- **可扩展**：通过 `Sink` 抽象接入任意通知渠道
- **不阻塞**：默认 `notify()` 不影响主流程；`send()` 也有 per-sink timeout 兜底
- **安全意识**：对 webhook 做限制；避免在日志中泄露敏感信息

## 非目标

- 不提供“统一的环境变量协议”（交由上层 integration 层决定）
- 不追求复杂的重试/队列/投递保证（可在上层或自定义 sink 中实现）

## 并发模型

当 `Hub::send(event).await` 执行时：

- 对每个 sink 生成一个并发任务
- 每个任务都被 `tokio::time::timeout(per_sink_timeout, ...)` 包裹
- 所有结果被 join 并聚合错误，最终以 `anyhow::Error` 返回

