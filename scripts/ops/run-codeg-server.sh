#!/usr/bin/env bash
set -euo pipefail

ENV_FILE="${CODEG_ENV_FILE:-/etc/codeg-server.env}"

if [[ -f "$ENV_FILE" ]]; then
  set -a
  # shellcheck disable=SC1090
  source "$ENV_FILE"
  set +a
fi

REPO_DIR="${CODEG_REPO_DIR:-/home/bob/codeg}"
HOST="${CODEG_HOST:-0.0.0.0}"
PORT="${CODEG_PORT:-3080}"
DATA_DIR="${CODEG_DATA_DIR:-/home/bob/.local/share/codeg}"
STATIC_DIR="${CODEG_STATIC_DIR:-$REPO_DIR/out}"
DISABLE_AUTH="${CODEG_DISABLE_AUTH:-1}"

if [[ -n "${CODEG_BINARY:-}" ]]; then
  BINARY="$CODEG_BINARY"
elif [[ -x "$REPO_DIR/src-tauri/target/release/codeg-server" ]]; then
  BINARY="$REPO_DIR/src-tauri/target/release/codeg-server"
elif [[ -x "$REPO_DIR/src-tauri/target/debug/codeg-server" ]]; then
  BINARY="$REPO_DIR/src-tauri/target/debug/codeg-server"
else
  echo "[codeg-server] no executable binary found under $REPO_DIR/src-tauri/target/{release,debug}" >&2
  exit 1
fi

if [[ ! -d "$STATIC_DIR" ]]; then
  echo "[codeg-server] static directory missing: $STATIC_DIR" >&2
  exit 1
fi

mkdir -p "$DATA_DIR"
cd "$REPO_DIR"

for user_bin_dir in "$HOME/.npm-global/bin" "$HOME/.local/bin" "$HOME/bin"; do
  if [[ -d "$user_bin_dir" && ":$PATH:" != *":$user_bin_dir:"* ]]; then
    PATH="$user_bin_dir:$PATH"
  fi
done

export CODEG_HOST="$HOST"
export CODEG_PORT="$PORT"
export CODEG_DATA_DIR="$DATA_DIR"
export CODEG_STATIC_DIR="$STATIC_DIR"
export CODEG_DISABLE_AUTH="$DISABLE_AUTH"
export PATH

exec "$BINARY"
