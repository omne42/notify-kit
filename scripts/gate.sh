#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"

has_rust=0
has_python=0
has_node=0

if [[ -f "$repo_root/Cargo.toml" ]]; then
  has_rust=1
fi
if [[ -f "$repo_root/pyproject.toml" || -f "$repo_root/requirements-dev.txt" ]]; then
  has_python=1
fi
if [[ -f "$repo_root/package.json" ]]; then
  has_node=1
fi

if [[ "$has_rust" -eq 0 && "$has_python" -eq 0 && "$has_node" -eq 0 ]]; then
  echo "gate: no supported project markers found (Cargo.toml / pyproject.toml / package.json); skipping." >&2
  exit 0
fi

if [[ "$has_rust" -eq 1 ]]; then
  echo "gate: rust (cargo fmt/check)" >&2
  (
    cd "$repo_root"
    cargo fmt --all -- --check
    cargo check --workspace --all-targets
  )
fi

if [[ "$has_python" -eq 1 ]]; then
  venv_python="$repo_root/.venv/bin/python"
  venv_ruff="$repo_root/.venv/bin/ruff"
  if [[ ! -x "$venv_python" || ! -x "$venv_ruff" ]]; then
    cat >&2 <<'EOF'
gate: python dev tools missing.

Run:
  ./scripts/bootstrap.sh
EOF
    exit 1
  fi

  echo "gate: python (ruff format/check + compileall)" >&2
  (
    cd "$repo_root"
    "$venv_ruff" format --check
    "$venv_ruff" check
    "$venv_python" -m compileall -q src
  )
fi

if [[ "$has_node" -eq 1 ]]; then
  if [[ ! -d "$repo_root/node_modules" ]]; then
    cat >&2 <<'EOF'
gate: node dependencies missing (node_modules/).

Run:
  npm install
or:
  ./scripts/bootstrap.sh
EOF
    exit 1
  fi

  echo "gate: node (npm run check)" >&2
  (
    cd "$repo_root"
    npm run -s check
  )
fi

# Optional: validate example bots syntax without installing deps.
if command -v node >/dev/null 2>&1 && [[ -d "$repo_root/bots" ]]; then
  bot_entrypoints=()
  while IFS= read -r f; do
    bot_entrypoints+=("$f")
  done < <(
    find "$repo_root/bots" -maxdepth 3 -type f \( -path "*/src/index.mjs" -o -path "*/src/index.js" \) -print 2>/dev/null
  )

  if [[ "${#bot_entrypoints[@]}" -gt 0 ]]; then
    echo "gate: node (bot syntax check)" >&2
    for f in "${bot_entrypoints[@]}"; do
      node --check "$f"
    done
  fi
fi
