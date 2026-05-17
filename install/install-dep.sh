#!/usr/bin/env bash
set -euo pipefail

# Mixer installer. Run as root on a Vultr / bare-metal Linux host.
#
# What it does, no source build, no toolchain required:
#   1. Installs basic dependencies needed by this installer.
#   2. Installs Docker Engine if missing.
#   3. Reads the published release manifest at GitHub Releases.
#   4. Detects host CPU architecture (x86_64 or arm64) and downloads the
#      matching pre-built `forge` binary.
#   5. Brings up SearXNG in Docker on a free local port.
#   6. Writes /etc/mixer/.env, preserving keys you have already filled in.
#   7. Installs a systemd unit so Mixer starts on boot and restarts on failure.
#
# Vultr startup-script notes:
#   - This script is intentionally non-interactive.
#   - Put your Vultr Serverless Inference API key in VULTR_INFERENCE_API_KEY below
#     before using this as a startup script, or inject it as an environment variable.
#   - SearXNG does NOT use an API key here. It runs locally in Docker and Mixer talks
#     to it through SEARXNG_URL=http://127.0.0.1:<random-port>.
#
# Important values to change before pasting into a Vultr startup script:
#   VULTR_INFERENCE_API_KEY="your-vultr-serverless-inference-api-key"
#   VULTR_DEFAULT_MODEL="your-vultr-model-name"
#
# Optional values:
#   APP_PORT="4590"
#   AI_PROVIDER="vultr"
#   VULTR_INFERENCE_BASE_URL="https://api.vultrinference.com/v1"

# User-tweakable defaults. Written to /etc/mixer/.env if not already present.
APP_PORT="${APP_PORT:-4590}"
AI_PROVIDER="${AI_PROVIDER:-vultr}"
VULTR_INFERENCE_API_KEY="${VULTR_INFERENCE_API_KEY:-}"
VULTR_INFERENCE_BASE_URL="${VULTR_INFERENCE_BASE_URL:-https://api.vultrinference.com/v1}"
VULTR_DEFAULT_MODEL="${VULTR_DEFAULT_MODEL:-}"

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

export DEBIAN_FRONTEND=noninteractive

echo "==> Mixer installer: install Docker, download release binary, set up SearXNG, install systemd service"

if [[ "${EUID}" -ne 0 ]]; then
  echo "ERROR: run this with sudo: sudo bash install-dep.sh"
  exit 1
fi

command_exists() { command -v "$1" >/dev/null 2>&1; }

apt_install_quiet() {
  apt-get update -qq
  apt-get install -y -qq "$@"
}

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

json_get() {
  python3 -c "
import json,sys
data = json.load(sys.stdin)
for key in sys.argv[1].split('.'):
    data = data[key]
print(data)
" "$1"
}

install_basic_dependencies() {
  if command_exists apt-get; then
    echo "==> Installing basic dependencies"
    apt_install_quiet ca-certificates curl gnupg python3 openssl lsb-release
  elif command_exists dnf; then
    echo "==> Installing basic dependencies"
    dnf install -y ca-certificates curl gnupg2 python3 openssl
  else
    echo "ERROR: unsupported distro. This installer supports apt or dnf based Linux systems."
    exit 1
  fi
}

install_docker() {
  echo "==> Docker is not installed. Installing Docker Engine + Compose plugin"

  if command_exists apt-get; then
    apt_install_quiet ca-certificates curl gnupg
    install -m 0755 -d /etc/apt/keyrings

    if [[ ! -f /etc/apt/keyrings/docker.gpg ]]; then
      curl -fsSL https://download.docker.com/linux/ubuntu/gpg \
        | gpg --dearmor -o /etc/apt/keyrings/docker.gpg
      chmod a+r /etc/apt/keyrings/docker.gpg
    fi

    . /etc/os-release
    DOCKER_DISTRO="ubuntu"
    DOCKER_CODENAME="${VERSION_CODENAME:-}"

    # Debian hosts need Docker's Debian repo instead of Ubuntu's.
    if [[ "${ID:-}" == "debian" ]]; then
      DOCKER_DISTRO="debian"
    fi

    if [[ -z "$DOCKER_CODENAME" ]]; then
      DOCKER_CODENAME="$(lsb_release -cs 2>/dev/null || true)"
    fi

    if [[ -z "$DOCKER_CODENAME" ]]; then
      echo "ERROR: could not detect distro codename for Docker apt repo."
      exit 1
    fi

    echo \
      "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.gpg] https://download.docker.com/linux/${DOCKER_DISTRO} ${DOCKER_CODENAME} stable" \
      > /etc/apt/sources.list.d/docker.list

    apt-get update -qq
    apt-get install -y -qq \
      docker-ce \
      docker-ce-cli \
      containerd.io \
      docker-buildx-plugin \
      docker-compose-plugin

  elif command_exists dnf; then
    dnf install -y dnf-plugins-core
    dnf config-manager --add-repo https://download.docker.com/linux/centos/docker-ce.repo
    dnf install -y \
      docker-ce \
      docker-ce-cli \
      containerd.io \
      docker-buildx-plugin \
      docker-compose-plugin
  else
    echo "ERROR: unsupported distro. This installer can auto-install Docker on apt/dnf systems only."
    exit 1
  fi

  systemctl enable --now docker
}

make_secret_key() {
  openssl rand -hex 32 2>/dev/null || python3 - <<'PY'
import secrets
print(secrets.token_hex(32))
PY
}

install_basic_dependencies

if ! command_exists docker; then
  install_docker
fi

if ! systemctl is-active --quiet docker; then
  echo "==> Starting Docker daemon"
  systemctl enable --now docker
fi

if ! docker info >/dev/null 2>&1; then
  echo "ERROR: Docker daemon is installed but not responding."
  journalctl -u docker --no-pager -n 80 || true
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
SEARXNG_SECRET_KEY="$(make_secret_key)"
echo "==> SearXNG will bind to ${SEARXNG_URL}"

cat > "$SEARXNG_DIR/settings.yml" <<EOF2
use_default_settings: true

server:
  secret_key: "${SEARXNG_SECRET_KEY}"
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
EOF2

cat > "$SEARXNG_DIR/docker-compose.yml" <<EOF2
services:
  searxng:
    image: searxng/searxng:latest
    container_name: ${CONTAINER_NAME}
    restart: unless-stopped
    ports:
      - "127.0.0.1:${SEARXNG_PORT}:8080"
    volumes:
      - ${SEARXNG_DIR}/settings.yml:/etc/searxng/settings.yml
EOF2

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
    -v "$SEARXNG_DIR/settings.yml:/etc/searxng/settings.yml" \
    searxng/searxng:latest >/dev/null
fi

echo "==> Waiting for SearXNG"
READY=0
for _ in {1..60}; do
  if curl -fsS "${SEARXNG_URL}/" >/dev/null 2>&1; then
    READY=1
    break
  fi
  sleep 1
done

if [[ "$READY" != "1" ]]; then
  echo "ERROR: SearXNG did not become ready."
  echo "Container logs:"
  docker logs "$CONTAINER_NAME" --tail=120 || true
  exit 1
fi

# Confirm JSON output is enabled, but do not fail the whole install just because an engine is slow.
if ! curl -fsS "${SEARXNG_URL}/search?q=test&format=json" >/dev/null 2>&1; then
  echo "WARNING: SearXNG homepage is up, but JSON search test failed. Mixer may still work after SearXNG finishes warming up."
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

# SearXNG is a local Docker service. It does not need an API key.
# Always refresh SEARXNG_URL because install just picked a fresh free port.
set_env_var "SEARXNG_URL" "$SEARXNG_URL"

# systemd unit.
echo "==> Creating systemd service ${SERVICE_FILE}"
cat > "$SERVICE_FILE" <<EOF2
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
EOF2

echo "==> Enabling and starting ${SERVICE_NAME}"
systemctl daemon-reload
systemctl enable --now "$SERVICE_NAME"
# open that port and etc
# so traffic can actually flow :(
sudo iptables -I INPUT -p tcp --dport ${APP_PORT} -j ACCEPT && sudo iptables-save | sudo tee /etc/iptables.rules >/dev/null

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
  echo "Set it before expecting Vultr inference to work, then restart:"
  echo "  sudo systemctl restart ${SERVICE_NAME}"
fi
