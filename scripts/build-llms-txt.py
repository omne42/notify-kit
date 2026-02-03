#!/usr/bin/env python3
from __future__ import annotations

import re
import sys
from pathlib import Path


def extract_summary_paths(summary_md: str) -> list[str]:
    # GitBook/mdBook style: * [Title](path/to/file.md)
    paths = re.findall(r"\[[^\]]+\]\(([^)]+\.md)\)", summary_md)
    # Deduplicate while preserving order.
    seen: set[str] = set()
    out: list[str] = []
    for p in paths:
        p = p.strip()
        if not p or p in seen:
            continue
        seen.add(p)
        out.append(p)
    return out


def append_file(out: list[str], label: str, path: Path) -> None:
    if not path.is_file():
        print(f"build-llms-txt: warning: missing {label}", file=sys.stderr)
        return
    out.append("\n---\n")
    out.append(f"## {label}\n\n")
    out.append(path.read_text(encoding="utf-8"))
    if not out[-1].endswith("\n"):
        out.append("\n")


def main() -> int:
    repo_root = Path(__file__).resolve().parents[1]
    output_path = Path(sys.argv[1]) if len(sys.argv) > 1 else repo_root / "llms.txt"

    summary_path = repo_root / "docs" / "SUMMARY.md"
    if not summary_path.is_file():
        print("build-llms-txt: missing docs/SUMMARY.md", file=sys.stderr)
        return 1

    summary_paths = extract_summary_paths(summary_path.read_text(encoding="utf-8"))

    parts: list[str] = []
    parts.append("# notify-kit\n\n")
    parts.append("This file is an LLM-friendly bundle of the `notify-kit` documentation and examples.\n\n")
    parts.append("- Source of truth: `docs/` + `bots/`\n")
    parts.append("- Regenerate: `./scripts/build-llms-txt.sh`\n\n")

    for rel in summary_paths:
        rel = rel.lstrip("./")
        append_file(parts, f"docs/{rel}", repo_root / "docs" / rel)

    append_file(parts, "bots/README.md", repo_root / "bots" / "README.md")
    for readme in sorted((repo_root / "bots").glob("*/README.md")):
        append_file(parts, str(readme.relative_to(repo_root)), readme)

    output_path.write_text("".join(parts), encoding="utf-8")
    print(f"build-llms-txt: wrote {output_path}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

