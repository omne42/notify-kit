#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"

if [[ ! -f "$repo_root/Cargo.toml" ]]; then
  echo "pre-commit-check: no Cargo.toml found; skipping rust strict checks." >&2
  exit 0
fi

echo "pre-commit-check: rust (clippy all-targets, deny warnings)" >&2
(
  cd "$repo_root"
  cargo clippy --workspace --all-targets -- -D warnings
)

echo "pre-commit-check: rust (strict production lints)" >&2
(
  cd "$repo_root"
  cargo clippy \
    --workspace \
    --all-features \
    --lib \
    --bins \
    --examples \
    -- \
    -D warnings \
    -W clippy::expect_used \
    -W clippy::let_underscore_must_use \
    -W clippy::map_clone \
    -W clippy::redundant_clone \
    -W clippy::unwrap_used
)
