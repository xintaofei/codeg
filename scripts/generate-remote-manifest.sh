#!/usr/bin/env bash
# Generates codeg-remote-manifest.json from per-platform .sha256/.size artifacts.
#
# Inputs (env):
#   VERSION       e.g. "v0.12.0"
#   GITHUB_REPO   e.g. "xintaofei/codeg"
#
# Inputs (filesystem):
#   artifacts/codeg-remote-{platform}.{tar.gz|zip}.sha256
#   artifacts/codeg-remote-{platform}.{tar.gz|zip}.size
#
# Output:
#   codeg-remote-manifest.json (in CWD)

set -euo pipefail

if [[ -z "${VERSION:-}" || -z "${GITHUB_REPO:-}" ]]; then
  echo "VERSION and GITHUB_REPO must be set" >&2
  exit 1
fi

VERSION_NO_V="${VERSION#v}"
GENERATED_AT="$(date -u +'%Y-%m-%dT%H:%M:%SZ')"

# Platforms we expect (must match release.yml matrix outputs)
declare -a PLATFORMS=(
  "linux-x64-musl:tar.gz"
  "linux-arm64-musl:tar.gz"
  "darwin-x64:tar.gz"
  "darwin-arm64:tar.gz"
  "windows-x64:zip"
)

ENTRIES_JSON=""
for entry in "${PLATFORMS[@]}"; do
  PLATFORM="${entry%:*}"
  EXT="${entry#*:}"
  ARTIFACT="codeg-remote-${PLATFORM}.${EXT}"
  SHA_FILE="artifacts/${ARTIFACT}.sha256"
  SIZE_FILE="artifacts/${ARTIFACT}.size"

  if [[ ! -f "${SHA_FILE}" || ! -f "${SIZE_FILE}" ]]; then
    echo "Missing meta for ${PLATFORM}: ${SHA_FILE} or ${SIZE_FILE}" >&2
    exit 1
  fi

  SHA="$(tr -d '[:space:]' < "${SHA_FILE}")"
  SIZE="$(tr -d '[:space:]' < "${SIZE_FILE}")"
  URL="https://github.com/${GITHUB_REPO}/releases/download/${VERSION}/${ARTIFACT}"
  EXEC_NAME="codeg-server"
  if [[ "${PLATFORM}" == windows-* ]]; then
    EXEC_NAME="codeg-server.exe"
  fi

  BINARY_JSON=$(cat <<EOF
    "${PLATFORM}": {
      "url": "${URL}",
      "sha256": "${SHA}",
      "size": ${SIZE},
      "exec_name": "${EXEC_NAME}"
    }
EOF
)
  if [[ -n "${ENTRIES_JSON}" ]]; then
    ENTRIES_JSON="${ENTRIES_JSON},
${BINARY_JSON}"
  else
    ENTRIES_JSON="${BINARY_JSON}"
  fi
done

cat > codeg-remote-manifest.json <<EOF
{
  "version": "${VERSION_NO_V}",
  "schema_version": "v3",
  "generated_at": "${GENERATED_AT}",
  "binaries": {
${ENTRIES_JSON}
  }
}
EOF

echo "Generated codeg-remote-manifest.json:"
cat codeg-remote-manifest.json
