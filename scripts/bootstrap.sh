#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

if [[ ! -d ".git" ]]; then
  git init -q
fi

bash ./scripts/setup-githooks.sh

if [[ -f "pyproject.toml" || -f "requirements-dev.txt" ]]; then
  if [[ ! -d ".venv" ]]; then
    python3 -m venv .venv
  fi
  ./.venv/bin/python -m pip install -U pip >/dev/null
  if [[ -f "requirements-dev.txt" ]]; then
    ./.venv/bin/pip install -r requirements-dev.txt >/dev/null
  fi
fi

if [[ -f "package.json" ]]; then
  npm install
fi

echo "bootstrap: ok" >&2

