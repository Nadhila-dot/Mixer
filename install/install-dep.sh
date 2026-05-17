#!/usr/bin/env bash
set -euo pipefail

################################################################################
# Mixer Vultr Startup Script
#
# Paste this into a Vultr startup script, or run it manually with:
#   sudo bash install.sh
#
# IMPORTANT:
#   This script is NON-INTERACTIVE.
#   You MUST edit the variables below before using it.
#
# REQUIRED CHANGES BEFORE RUNNING:
#
#   1. VULTR_INFERENCE_API_KEY
#      Put your Vultr Serverless Inference API key here.
#
#   2. VULTR_DEFAULT_MODEL
#      Put the model name you want Mixer to use by default.
#
#   3. REPO_URL
#      Put your public GitHub repo URL here.
#
# OPTIONAL CHANGES:
#
#   APP_PORT
#      Port Mixer will run on. Default: 3000
#
#   INSTALL_BRANCH
#      Git branch to deploy. Default: main
#
#   SERVICE_NAME
#      systemd service name. Default: mixer
#
#   INSTALL_BIN
#      Where the built binary is installed. Default: /usr/local/bin/forge
#
# NOTES:
#   - Docker must be available or this script will install it.
#   - SearXNG runs locally in Docker on a random free localhost port.
#   - The selected SearXNG URL is written to /etc/mixer/.env.
#   - The app env file lives at /etc/mixer/.env.
#   - This script assumes the Rust binary is named "forge".
################################################################################

################################################################################
# EDIT THESE VALUES
################################################################################

REPO_URL="https://github.com/Nadhila-dot/Mixer.git"
INSTALL_BRANCH="main"

AI_PROVIDER="vultr"
VULTR_INFERENCE_API_KEY="PASTE_YOUR_VULTR_SERVERLESS_INFERENCE_API_KEY_HERE"
VULTR_INFERENCE_BASE_URL="https://api.vultrinference.com/v1"
VULTR_DEFAULT_MODEL="PASTE_YOUR_VULTR_MODEL_NAME_HERE"

APP_PORT="3000"

################################################################################
# ADVANCED SETTINGS usually do not need changing
################################################################################

MIXER_DIR="/etc/mixer"
APP_DIR="$MIXER_DIR/app"
SEARXNG_DIR="$MIXER_DIR/searxng"
ENV_FILE="$MIXER_DIR/.env"

CONTAINER_NAME="mixer-searxng"

SERVICE_NAME="mixer"
SERVICE_FILE="/etc/systemd/system/${SERVICE_NAME}.service"

INSTALL_BIN="/usr/local/bin/forge"
ROOT_HOME="/root"

################################################################################
# INTERNALS
################################################################################

echo ""
echo "============================================================"
echo " Mixer Vultr startup installer"
echo "============================================================"
echo ""

if [[ "${EUID}" -ne 0 ]]; then
  echo "ERROR: this script must run as root."
  echo "Use sudo or run it as a Vultr startup script."
  exit 1
fi

if [[ -z "$REPO_URL" || "$REPO_URL" == "https://github.com/YOUR_USERNAME/YOUR_REPO.git" ]]; then
  echo "ERROR: REPO_URL is not configured."
  echo "Edit REPO_URL at the top of this script."
  exit 1
fi

if [[ -z "$VULTR_INFERENCE_API_KEY" || "$VULTR_INFERENCE_API_KEY" == "PASTE_YOUR_VULTR_SERVERLESS_INFERENCE_API_KEY_HERE" ]]; then
  echo "ERROR: VULTR_INFERENCE_API_KEY is not configured."
  echo "Edit VULTR_INFERENCE_API_KEY at the top of this script."
  exit 1
fi

if [[ -z "$VULTR_DEFAULT_MODEL" || "$VULTR_DEFAULT_MODEL" == "PASTE_YOUR_VULTR_MODEL_NAME_HERE" ]]; then
  echo "ERROR: VULTR_DEFAULT_MODEL is not configured."
  echo "Edit VULTR_DEFAULT_MODEL at the top of this script."
  exit 1
fi

command_exists() {
  command -v "$1" >/dev/null 2>&1
}

install_base_packages() {
  echo "==> Installing base packages"

  export DEBIAN_FRONTEND=noninteractive

  apt-get update
  apt-get install -y \
    ca-certificates \
    curl \
    git \
    python3 \
    build-essential \
    pkg-config \
    libssl-dev
}

install_docker() {
  if command_exists docker; then
    echo "==> Docker already installed"
    return 0
  fi

  echo "==> Installing Docker"
  curl -fsSL https://get.docker.com | sh

  systemctl enable docker
  systemctl start docker
}

install_rust_toolchain() {
  if command_exists cargo && command_exists rustc; then
    echo "==> Rust already installed"
    return 0
  fi

  echo "==> Installing Rust toolchain"
  curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable --no-modify-path

  export PATH="${ROOT_HOME}/.cargo/bin:${PATH}"

  if [[ -f "${ROOT_HOME}/.cargo/env" ]]; then
    # shellcheck disable=SC1091
    source "${ROOT_HOME}/.cargo/env"
  fi
}

install_bun() {
  if command_exists bun; then
    echo "==> Bun already installed"
    return 0
  fi

  echo "==> Installing Bun"
  curl -fsSL https://bun.sh/install | bash

  export PATH="${ROOT_HOME}/.bun/bin:${ROOT_HOME}/.cargo/bin:${PATH}"
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

  if [[ -n "$ip" ]]; then
    echo "$ip"
    return 0
  fi

  return 1
}

set_env_var() {
  local key="$1"
  local value="$2"

  touch "$ENV_FILE"

  if grep -q "^${key}=" "$ENV_FILE"; then
    sed -i "s|^${key}=.*|${key}=${value}|" "$ENV_FILE"
  else
    echo "${key}=${value}" >> "$ENV_FILE"
  fi
}

ensure_env_var_exists() {
  local key="$1"
  local default_value="${2:-}"

  touch "$ENV_FILE"

  if ! grep -q "^${key}=" "$ENV_FILE"; then
    echo "${key}=${default_value}" >> "$ENV_FILE"
  fi
}

echo "==> Preparing system"
install_base_packages
install_docker
install_rust_toolchain
install_bun

export PATH="${ROOT_HOME}/.bun/bin:${ROOT_HOME}/.cargo/bin:${PATH}"

if ! docker info >/dev/null 2>&1; then
  echo "ERROR: Docker daemon is not running."
  exit 1
fi

echo "==> Creating Mixer directories"
mkdir -p "$MIXER_DIR"
mkdir -p "$SEARXNG_DIR"

echo "==> Cloning/updating Mixer repo"

if [[ -d "$APP_DIR/.git" ]]; then
  cd "$APP_DIR"
  git fetch origin "$INSTALL_BRANCH"
  git reset --hard "origin/${INSTALL_BRANCH}"
else
  rm -rf "$APP_DIR"
  git clone --branch "$INSTALL_BRANCH" "$REPO_URL" "$APP_DIR"
  cd "$APP_DIR"
fi

SEARXNG_PORT="$(find_free_port)"
SEARXNG_URL="http://127.0.0.1:${SEARXNG_PORT}"

echo "==> Selected SearXNG port: ${SEARXNG_PORT}"

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

echo "==> Stopping old SearXNG container if present"
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

for i in {1..30}; do
  if curl -fsS "${SEARXNG_URL}/search?q=test&format=json" >/dev/null 2>&1; then
    READY=1
    break
  fi

  sleep 1
done

if [[ "$READY" != "1" ]]; then
  echo "ERROR: SearXNG did not become ready."
  echo ""
  echo "Container logs:"
  docker logs "$CONTAINER_NAME" --tail=100 || true
  exit 1
fi

echo "==> Writing environment file: $ENV_FILE"

touch "$ENV_FILE"
chmod 600 "$ENV_FILE"

set_env_var "AI_PROVIDER" "$AI_PROVIDER"
set_env_var "VULTR_INFERENCE_API_KEY" "$VULTR_INFERENCE_API_KEY"
set_env_var "VULTR_INFERENCE_BASE_URL" "$VULTR_INFERENCE_BASE_URL"
set_env_var "VULTR_DEFAULT_MODEL" "$VULTR_DEFAULT_MODEL"
set_env_var "PORT" "$APP_PORT"
set_env_var "SEARXNG_URL" "$SEARXNG_URL"

ensure_env_var_exists "BRAVE_SEARCH_API_KEY" ""
ensure_env_var_exists "GOOGLE_SEARCH_API_KEY" ""
ensure_env_var_exists "GOOGLE_SEARCH_ENGINE_ID" ""

echo "==> Linking app .env to /etc/mixer/.env"

if [[ -e "$APP_DIR/.env" && ! -L "$APP_DIR/.env" ]]; then
  cp "$APP_DIR/.env" "$APP_DIR/.env.backup.$(date +%s)"
  rm -f "$APP_DIR/.env"
fi

ln -sfn "$ENV_FILE" "$APP_DIR/.env"

echo "==> Building Mixer release binary"
cd "$APP_DIR"

bun install --frozen-lockfile
cargo build --locked --release

if [[ ! -f "$APP_DIR/target/release/forge" ]]; then
  echo "ERROR: release binary not found:"
  echo "  $APP_DIR/target/release/forge"
  echo ""
  echo "Check that your Cargo package builds a binary named 'forge'."
  exit 1
fi

echo "==> Installing binary to $INSTALL_BIN"
install -m 755 "$APP_DIR/target/release/forge" "$INSTALL_BIN"

echo "==> Creating systemd service: $SERVICE_FILE"

cat > "$SERVICE_FILE" <<EOF
[Unit]
Description=Mixer application service
After=network-online.target docker.service
Wants=network-online.target docker.service

[Service]
Type=simple
WorkingDirectory=$APP_DIR
EnvironmentFile=$ENV_FILE
ExecStart=$INSTALL_BIN
Restart=always
RestartSec=3

[Install]
WantedBy=multi-user.target
EOF

echo "==> Enabling and starting Mixer service"

systemctl daemon-reload
systemctl enable --now "$SERVICE_NAME"

PUBLIC_IPV4="$(get_public_ipv4 || true)"

echo ""
echo "============================================================"
echo " Mixer install complete"
echo "============================================================"
echo ""
echo "App directory:"
echo "  $APP_DIR"
echo ""
echo "Environment file:"
echo "  $ENV_FILE"
echo ""
echo "SearXNG:"
echo "  $SEARXNG_URL"
echo ""
echo "Systemd:"
echo "  systemctl status $SERVICE_NAME"
echo "  journalctl -u $SERVICE_NAME -f"
echo ""
echo "SearXNG logs:"
echo "  docker logs $CONTAINER_NAME -f"
echo ""
if [[ -n "${PUBLIC_IPV4}" ]]; then
  echo "App URL:"
  echo "  http://${PUBLIC_IPV4}:${APP_PORT}"
else
  echo "App URL:"
  echo "  http://<your-public-ip>:${APP_PORT}"
fi
echo ""