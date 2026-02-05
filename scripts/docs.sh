#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"

if ! command -v mdbook >/dev/null 2>&1; then
  cat >&2 <<'EOF'
docs: mdbook not found.

Install:
  cargo install mdbook --locked

Then run:
  ./scripts/docs.sh serve
EOF
  exit 1
fi

cmd="${1:-serve}"
case "$cmd" in
  serve)
    shift || true
    mdbook serve "$repo_root/docs" "$@"
    ;;
  build)
    shift || true
    mdbook build "$repo_root/docs" "$@"
    ;;
  test)
    shift || true
    docs_target_dir="$repo_root/target/mdbook-test"
    rm -rf "$docs_target_dir"
    (
      cd "$repo_root"
      CARGO_TARGET_DIR="$docs_target_dir" cargo build -p notify-kit
    )
    mdbook test -L "$docs_target_dir/debug/deps" "$@" "$repo_root/docs"
    ;;
  *)
    cat >&2 <<'EOF'
Usage:
  ./scripts/docs.sh serve [mdbook args...]   # local preview with search
  ./scripts/docs.sh build [mdbook args...]   # build to target/mdbook/
  ./scripts/docs.sh test  [mdbook args...]   # compile Rust code snippets (requires cargo build)
EOF
    exit 2
    ;;
esac
