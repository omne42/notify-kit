#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"

if ! command -v mdbook >/dev/null 2>&1; then
  cat >&2 <<'EOF'
docs: mdbook not found.

Install:
  cargo install mdbook

Then run:
  ./scripts/docs.sh serve
EOF
  exit 1
fi

cmd="${1:-serve}"
case "$cmd" in
  serve)
    mdbook serve "$repo_root/docs"
    ;;
  build)
    mdbook build "$repo_root/docs"
    ;;
  test)
    mdbook test "$repo_root/docs"
    ;;
  *)
    cat >&2 <<'EOF'
Usage:
  ./scripts/docs.sh serve   # local preview with search
  ./scripts/docs.sh build   # build to target/mdbook/
  ./scripts/docs.sh test    # mdbook link checks
EOF
    exit 2
    ;;
esac

