# 开发

## 质量门禁

离线检查：

```bash
CARGO_NET_OFFLINE=true ./scripts/gate.sh
```

常用命令：

```bash
cargo fmt --all
cargo test --workspace
```

## 目录结构

- `crates/notify-kit/`：库实现
- `docs/`：GitBook 文档（本目录）
- `scripts/gate.sh`：格式化/编译门禁

## 文档维护

- 改动文档：直接编辑 `docs/*.md`
- 目录结构：编辑 `docs/SUMMARY.md`
- 如果你使用 GitBook：把 Book root 指向 `docs/`
