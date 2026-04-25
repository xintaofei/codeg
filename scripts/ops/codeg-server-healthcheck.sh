#!/usr/bin/env bash
set -euo pipefail

ENV_FILE="${CODEG_ENV_FILE:-/etc/codeg-server.env}"

if [[ -f "$ENV_FILE" ]]; then
  set -a
  # shellcheck disable=SC1090
  source "$ENV_FILE"
  set +a
fi

SERVICE_NAME="${CODEG_SYSTEMD_SERVICE:-codeg-server.service}"
PORT="${CODEG_PORT:-3080}"
HEALTH_URL="${CODEG_HEALTH_URL:-http://127.0.0.1:${PORT}/api/health}"

if ! systemctl is-active --quiet "$SERVICE_NAME"; then
  logger -t codeg-server-healthcheck "service inactive, starting ${SERVICE_NAME}"
  exec systemctl restart "$SERVICE_NAME"
fi

if curl --fail --silent --show-error --max-time 10 -X POST "$HEALTH_URL" | grep -q '"status":"ok"'; then
  exit 0
fi

logger -t codeg-server-healthcheck "healthcheck failed for ${HEALTH_URL}, restarting ${SERVICE_NAME}"
exec systemctl restart "$SERVICE_NAME"
