#!/usr/bin/env bash
set -euo pipefail

# Mixer installer. Run as root on a Vultr / bare-metal Linux host.
#
# What it does, no source build, no toolchain required:
#   1. Reads the published release manifest at GitHub Releases.
#   2. Detects host CPU architecture (x86_64 or arm64) and downloads the
#      matching pre-built `forge` binary.
#   3. Brings up SearXNG in Docker on a free local port.
#   4. Writes /etc/mixer/.env (preserving keys you have already filled in).
#   5. Installs a systemd unit so Mixer starts on boot and restarts on failure.
#
# Set VULTR_INFERENCE_API_KEY (and friends) before running, either as env
# vars or by editing the defaults block below. The script will warn if it
# is left empty.

# User-tweakable defaults. Written to /etc/mixer/.env if not already present.
APP_PORT="${APP_PORT:-3000}"
VULTR_INFERENCE_API_KEY="${VULTR_INFERENCE_API_KEY:-}"
VULTR_INFERENCE_BASE_URL="${VULTR_INFERENCE_BASE_URL:-https://api.vultrinference.com/v1}"
VULTR_DEFAULT_MODEL="${VULTR_DEFAULT_MODEL:-}"
AI_PROVIDER="${AI_PROVIDER:-vultr}"

# Release manifest. Defaults to "latest". Override MIXER_RELEASE_JSON_URL to
# pin a tag, e.g.
#   MIXER_RELEASE_JSON_URL=https://github.com/Nadhila-dot/Mixer/releases/download/v0.2/release.json
MIXER_RELEASE_JSON_URL="${MIXER_RELEASE_JSON_URL:-https://github.com/Nadhila-dot/Mixer/releases/latest/download/release.json}"

# Fixed install paths.
MIXER_DIR="/etc/mixer"
SEARXNG_DIR="$MIXER_DIR/searxng"
ENV_FILE="$MIXER_DIR/.env"
CONTAINER_NAME="mixer-searxng"
SERVICE_NAME="mixer"
SERVICE_FILE="/etc/systemd/system/${SERVICE_NAME}.service"
INSTALL_BIN="/usr/local/bin/forge"

echo "==> Mixer installer: download release binary, install systemd service, set up SearXNG"

if [[ "${EUID}" -ne 0 ]]; then
  echo "ERROR: run this with sudo: sudo bash install-dep.sh"
  exit 1
fi

command_exists() { command -v "$1" >/dev/null 2>&1; }

find_free_port() {
  python3 - <<'PY'
import socket
with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
    s.bind(("127.0.0.1", 0))
    print(s.getsockname()[1])
PY
}

get_public_ipv4() {
  local ip=""
  for url in \
    https://api.ipify.org \
    https://ifconfig.io/ip \
    https://ifconfig.me/ip \
    https://ipinfo.io/ip
  do
    ip="$(curl -4 -fsS "$url" 2>/dev/null | tr -d '[:space:]' || true)"
    if [[ "$ip" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
      echo "$ip"
      return 0
    fi
  done
  ip="$(hostname -I 2>/dev/null | awk '{print $1}' || true)"
  [[ -n "$ip" ]] && { echo "$ip"; return 0; }
  return 1
}

set_env_var() {
  local key="$1" value="$2"
  touch "$ENV_FILE"
  if grep -q "^${key}=" "$ENV_FILE"; then
    sed -i "s|^${key}=.*|${key}=${value}|" "$ENV_FILE"
  else
    echo "${key}=${value}" >> "$ENV_FILE"
  fi
}

ensure_env_var() {
  local key="$1" default_value="${2:-}"
  touch "$ENV_FILE"
  if ! grep -q "^${key}=" "$ENV_FILE"; then
    echo "${key}=${default_value}" >> "$ENV_FILE"
  fi
}

# Reads a dot-path field from release.json (stdin) via python3.
json_get() {
  python3 -c "
import json,sys
data = json.load(sys.stdin)
for key in sys.argv[1].split('.'):
    data = data[key]
print(data)
" "$1"
}

# Make sure the basic tooling is on the box before we go any further.
if ! command_exists curl; then
  echo "==> Installing curl"
  apt-get update -qq && apt-get install -y -qq curl
fi

if ! command_exists python3; then
  echo "==> Installing python3 (needed to parse release.json)"
  apt-get update -qq && apt-get install -y -qq python3
fi

if ! command_exists docker; then
  echo "ERROR: Docker is not installed. Install Docker first, then rerun this script."
  exit 1
fi

if ! docker info >/dev/null 2>&1; then
  echo "ERROR: Docker daemon is not running."
  exit 1
fi

# Detect arch and pick the matching binary key in release.json.
RAW_ARCH="$(uname -m)"
case "$RAW_ARCH" in
  x86_64|amd64)   ARCH_KEY="linux_x86_64" ;;
  aarch64|arm64)  ARCH_KEY="linux_arm64"  ;;
  *)
    echo "ERROR: unsupported CPU architecture: ${RAW_ARCH}"
    echo "Mixer publishes binaries for x86_64 and arm64 only."
    exit 1
    ;;
esac

echo "==> Detected architecture: ${RAW_ARCH} (using ${ARCH_KEY} build)"
echo "==> Fetching release manifest: ${MIXER_RELEASE_JSON_URL}"

RELEASE_JSON="$(curl -fsSL "$MIXER_RELEASE_JSON_URL")" || {
  echo "ERROR: could not download release manifest from ${MIXER_RELEASE_JSON_URL}"
  exit 1
}

BINARY_URL="$(echo "$RELEASE_JSON" | json_get "downloads.${ARCH_KEY}.url")" || {
  echo "ERROR: release.json does not contain downloads.${ARCH_KEY}.url"
  echo "Manifest contents:"
  echo "$RELEASE_JSON"
  exit 1
}

RELEASE_TAG="$(echo "$RELEASE_JSON" | json_get "tag" 2>/dev/null || echo "unknown")"
echo "==> Mixer release: ${RELEASE_TAG}"
echo "==> Binary URL:    ${BINARY_URL}"

# /etc/mixer layout and the SearXNG sidecar.
echo "==> Creating ${MIXER_DIR} layout"
mkdir -p "$SEARXNG_DIR"

SEARXNG_PORT="$(find_free_port)"
SEARXNG_URL="http://127.0.0.1:${SEARXNG_PORT}"
echo "==> SearXNG will bind to ${SEARXNG_URL}"

cat > "$SEARXNG_DIR/settings.yml" <<'EOF'
use_default_settings: true

server:
  limiter: false
  image_proxy: true

search:
  safe_search: 0
  autocomplete: ""

ui:
  static_use_hash: true

formats:
  - html
  - json
EOF

cat > "$SEARXNG_DIR/docker-compose.yml" <<EOF
services:
  searxng:
    image: searxng/searxng:latest
    container_name: ${CONTAINER_NAME}
    restart: unless-stopped
    ports:
      - "127.0.0.1:${SEARXNG_PORT}:8080"
    volumes:
      - ${SEARXNG_DIR}/settings.yml:/etc/searxng/settings.yml:ro
EOF

echo "==> Stopping any existing SearXNG container"
docker rm -f "$CONTAINER_NAME" >/dev/null 2>&1 || true

echo "==> Starting SearXNG"
cd "$SEARXNG_DIR"
if docker compose version >/dev/null 2>&1; then
  docker compose up -d
elif command_exists docker-compose; then
  docker-compose up -d
else
  docker run -d \
    --name "$CONTAINER_NAME" \
    --restart unless-stopped \
    -p "127.0.0.1:${SEARXNG_PORT}:8080" \
    -v "$SEARXNG_DIR/settings.yml:/etc/searxng/settings.yml:ro" \
    searxng/searxng:latest >/dev/null
fi

echo "==> Waiting for SearXNG"
READY=0
for _ in {1..30}; do
  if curl -fsS "${SEARXNG_URL}/search?q=test&format=json" >/dev/null 2>&1; then
    READY=1
    break
  fi
  sleep 1
done

if [[ "$READY" != "1" ]]; then
  echo "ERROR: SearXNG did not become ready."
  echo "Container logs:"
  docker logs "$CONTAINER_NAME" --tail=100 || true
  exit 1
fi

# Download the Mixer binary, verify it is an ELF, install to /usr/local/bin.
echo "==> Downloading Mixer binary"
TMP_BIN="$(mktemp)"
trap 'rm -f "$TMP_BIN"' EXIT

curl -fsSL -o "$TMP_BIN" "$BINARY_URL" || {
  echo "ERROR: failed to download binary from ${BINARY_URL}"
  exit 1
}

# Magic-byte sanity check so we do not install an HTML error page as a binary.
if ! head -c 4 "$TMP_BIN" | grep -q $'\x7fELF'; then
  echo "ERROR: downloaded file at ${BINARY_URL} is not an ELF binary."
  echo "First bytes:"
  head -c 200 "$TMP_BIN" | cat -v
  exit 1
fi

echo "==> Installing binary to ${INSTALL_BIN}"
install -m 755 "$TMP_BIN" "$INSTALL_BIN"
rm -f "$TMP_BIN"
trap - EXIT

# Write /etc/mixer/.env, preserving any keys the operator already set.
echo "==> Writing ${ENV_FILE}"
touch "$ENV_FILE"
chmod 600 "$ENV_FILE"

ensure_env_var "AI_PROVIDER" "$AI_PROVIDER"
ensure_env_var "VULTR_INFERENCE_API_KEY" "$VULTR_INFERENCE_API_KEY"
ensure_env_var "VULTR_INFERENCE_BASE_URL" "$VULTR_INFERENCE_BASE_URL"
ensure_env_var "VULTR_DEFAULT_MODEL" "$VULTR_DEFAULT_MODEL"
ensure_env_var "PORT" "$APP_PORT"
ensure_env_var "BRAVE_SEARCH_API_KEY" ""
ensure_env_var "GOOGLE_SEARCH_API_KEY" ""
ensure_env_var "GOOGLE_SEARCH_ENGINE_ID" ""

# Always refresh SEARXNG_URL because install just picked a fresh free port.
set_env_var "SEARXNG_URL" "$SEARXNG_URL"

# systemd unit.
echo "==> Creating systemd service ${SERVICE_FILE}"
cat > "$SERVICE_FILE" <<EOF
[Unit]
Description=Mixer agent server
After=network-online.target docker.service
Wants=network-online.target docker.service

[Service]
Type=simple
WorkingDirectory=${MIXER_DIR}
EnvironmentFile=${ENV_FILE}
ExecStart=${INSTALL_BIN}
Restart=always
RestartSec=3

[Install]
WantedBy=multi-user.target
EOF

echo "==> Enabling and starting ${SERVICE_NAME}"
systemctl daemon-reload
systemctl enable --now "$SERVICE_NAME"

PUBLIC_IPV4="$(get_public_ipv4 || true)"

echo ""
echo "Done."
echo ""
echo "Release:    ${RELEASE_TAG} (${ARCH_KEY})"
echo "Binary:     ${INSTALL_BIN}"
echo "Config:     ${ENV_FILE}"
echo "SearXNG:    ${SEARXNG_URL}"
echo "Service:    systemctl status ${SERVICE_NAME}"
echo ""
if [[ -n "${PUBLIC_IPV4}" ]]; then
  echo "Access Mixer at: http://${PUBLIC_IPV4}:${APP_PORT}"
else
  echo "Access Mixer at: http://<your-public-ip>:${APP_PORT}"
fi
echo ""
if [[ -z "$VULTR_INFERENCE_API_KEY" ]]; then
  echo "Reminder: VULTR_INFERENCE_API_KEY is empty in ${ENV_FILE}."
  echo "Set it (and any other keys) then restart:"
  echo "  sudo systemctl restart ${SERVICE_NAME}"
fi
