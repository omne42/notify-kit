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

## 本地预览（mdBook）

本目录的结构兼容 mdBook（使用 `SUMMARY.md` 作为目录）。你可以用 mdBook 本地预览（含搜索）：

```bash
./scripts/docs.sh serve
```

传参示例（容器/远程访问）：

```bash
./scripts/docs.sh serve --hostname 0.0.0.0 --port 3000
```

首次使用需要安装：

```bash
cargo install mdbook
```

## LLM 友好文档（llms.txt）

为了让 LLM/agent 更容易“看懂仓库文档”，我们提供了一个聚合文件：`llms.txt`。

更新后请重新生成：

```bash
./scripts/build-llms-txt.sh
```
