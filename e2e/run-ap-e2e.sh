#!/usr/bin/env bash
# AP E2E Test Runner
#
# Orchestrates the full ActivityPub E2E test workflow:
#   1. Generate self-signed certificates
#   2. Build Emumet with test-mode features
#   3. Start Docker Compose infrastructure (postgres, redis, kratos, hydra, nginx, iceshrimp)
#   4. Wait for all services to become healthy
#   5. Start Emumet server (host process)
#   6. Run mock peer E2E test suite
#   7. Run Iceshrimp federation E2E test suite
#   8. Cleanup
#
# Usage:
#   bash e2e/run-ap-e2e.sh
#
# Environment:
#   Uses .env for base configuration, overrides for AP E2E mode.
#
# Required tools:
#   - docker (compose v2 plugin)
#   - cargo
#   - openssl

set -euo pipefail

# ── Colors ──────────────────────────────────────────────────────────────────
GREEN='\033[0;32m'; RED='\033[0;31m'; YELLOW='\033[0;33m'; NC='\033[0m'
info()  { echo -e "${GREEN}[INFO]${NC} $1"; }
warn()  { echo -e "${YELLOW}[WARN]${NC} $1"; }
err()   { echo -e "${RED}[ERROR]${NC} $1"; }

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$ROOT_DIR"

# PID placeholder — declared before `trap` so `set -u` does not crash cleanup
SERVER_PID=""

# ── 0. Prerequisites ────────────────────────────────────────────────────────
info "Checking prerequisites..."
command -v docker >/dev/null 2>&1 || { err "docker not found"; exit 1; }
command -v cargo >/dev/null 2>&1 || { err "cargo not found"; exit 1; }
command -v openssl >/dev/null 2>&1 || { err "openssl not found"; exit 1; }
command -v curl >/dev/null 2>&1 || { err "curl not found (required for readiness checks)"; exit 1; }

# Detect compose subcommand (compose v2 plugin or legacy docker-compose)
if docker compose version >/dev/null 2>&1; then
    COMPOSE_CMD="docker compose"
elif command -v docker-compose >/dev/null 2>&1; then
    COMPOSE_CMD="docker-compose"
else
    err "docker compose (v2) or docker-compose not found"
    exit 1
fi
info "Using: $COMPOSE_CMD"

# Detect host IP for rootless Docker (host.docker.internal doesn't resolve correctly)
HOST_IP=$(ip -4 route get 1 | head -1 | awk '{print $7}')
info "Detected host IP: $HOST_IP"

# ── 1. Certificates ────────────────────────────────────────────────────────
info "Generating self-signed certificates for nip.io domains..."
bash e2e/certs/gen-cert.sh

# ── 2. Master key password ─────────────────────────────────────────────────
if [ ! -f master-key-password ]; then
    info "Creating master-key-password..."
    echo "develop" > master-key-password
    chmod 600 master-key-password
    info "  Created: master-key-password (dev password)"
fi

# ── 3. Source .env (base config) ───────────────────────────────────────────
if [ -f .env ]; then
    info "Loading .env configuration..."
    set -o allexport
    source .env
    set +o allexport
else
    warn ".env not found — relying on existing environment"
fi

# ── 4. Build server ────────────────────────────────────────────────────────
info "Building Emumet server with test-mode features..."
cargo build -p server --features test-mode

# ── 5. Start compose infrastructure ────────────────────────────────────────
info "Starting Docker Compose infrastructure..."
info "  compose files: compose.yml + compose.ap-e2e.yml"
info "  profile: ap-e2e"

# Ensure cleanup on exit
cleanup() {
    info "Cleaning up..."
    kill $SERVER_PID 2>/dev/null || true
    wait $SERVER_PID 2>/dev/null || true
    rm -f "$COMPOSE_OVERRIDE"
    $COMPOSE_CMD -f compose.yml -f compose.ap-e2e.yml --profile ap-e2e down
    info "Cleanup complete"
}
trap cleanup EXIT

# Rootless Docker workaround: create temp override with correct host IP for extra_hosts
COMPOSE_OVERRIDE=$(mktemp)
cat > "$COMPOSE_OVERRIDE" <<EOF
services:
  nginx:
    extra_hosts:
      - "host.docker.internal:${HOST_IP}"
EOF
$COMPOSE_CMD -f compose.yml -f compose.ap-e2e.yml -f "$COMPOSE_OVERRIDE" --profile ap-e2e up -d

# ── 6. Wait for services ───────────────────────────────────────────────────
info "Waiting for infrastructure services..."

echo "  - postgres..."
until $COMPOSE_CMD exec postgres pg_isready -U postgres 2>/dev/null; do
    sleep 2
done
echo "    postgres is ready"

echo "  - kratos..."
until curl -sf http://localhost:4433/health/alive >/dev/null 2>&1; do
    sleep 2
done
echo "    kratos is ready"

echo "  - hydra..."
until curl -sf http://localhost:4444/health/alive >/dev/null 2>&1; do
    sleep 2
done
echo "    hydra is ready"

echo "  - redis..."
until $COMPOSE_CMD exec redis redis-cli ping 2>/dev/null | grep -q PONG; do
    sleep 2
done
echo "    redis is ready"

echo "  - nginx (port 8443)..."
until echo >/dev/tcp/localhost/8443 2>/dev/null; do
    sleep 2
done
echo "    nginx is accepting connections"

echo "  - iceshrimp..."
until curl -sk https://iceshrimp.127.0.0.1.nip.io:8443/api/endpoint >/dev/null 2>&1; do
    sleep 2
done
echo "    iceshrimp is ready"

# ── 7. Set AP E2E environment ──────────────────────────────────────────────
# These override any values from .env
export AP_TEST_ALLOWED_FETCH_HOSTS="127.0.0.1,iceshrimp.127.0.0.1.nip.io"
export AP_TEST_ACCEPT_INVALID_CERTS="1"
export EMUMET_E2E_EXTERNAL_SERVER="1"
export EMUMET_E2E_SERVER_BASE_URL="https://emumet.127.0.0.1.nip.io:8443"
export EMUMET_E2E_PUBLIC_BASE_URL="https://emumet.127.0.0.1.nip.io:8443"
export PUBLIC_BASE_URL="https://emumet.127.0.0.1.nip.io:8443"
export ICESHRIMP_BASE_URL="https://iceshrimp.127.0.0.1.nip.io:8443"
if [ -z "${EMUMET_TEST_MODE_TOKEN:-}" ]; then
    EMUMET_TEST_MODE_TOKEN=$(openssl rand -hex 32)
    info "Generated random EMUMET_TEST_MODE_TOKEN"
else
    info "Using existing EMUMET_TEST_MODE_TOKEN"
fi
export EMUMET_TEST_MODE_TOKEN

info "AP E2E environment configured:"
echo "  AP_TEST_ALLOWED_FETCH_HOSTS=$AP_TEST_ALLOWED_FETCH_HOSTS"
echo "  EMUMET_E2E_EXTERNAL_SERVER=$EMUMET_E2E_EXTERNAL_SERVER"
echo "  EMUMET_E2E_SERVER_BASE_URL=$EMUMET_E2E_SERVER_BASE_URL"
echo "  PUBLIC_BASE_URL=$PUBLIC_BASE_URL"
echo "  ICESHRIMP_BASE_URL=$ICESHRIMP_BASE_URL"
echo "  EMUMET_TEST_MODE_TOKEN=(set — value not logged)"

# ── 8. Start Emumet server ─────────────────────────────────────────────────
info "Starting Emumet server in test-mode (host process)..."
cargo run -p server --features test-mode &
SERVER_PID=$!
info "  Server PID: $SERVER_PID"

# Wait for server to be ready (via test-mode health endpoint)
info "Waiting for Emumet server..."
for i in $(seq 1 30); do
    if curl -skf "https://emumet.127.0.0.1.nip.io:8443/__test__/health" >/dev/null 2>&1; then
        info "  Emumet server is ready (after ${i}s)"
        break
    fi
    if [ $i -eq 30 ]; then
        err "Emumet server failed to start within 60 seconds"
        kill $SERVER_PID 2>/dev/null
        exit 1
    fi
    sleep 2
done

# ── 9. Run mock AP E2E tests ───────────────────────────────────────────────
info "Running mock AP E2E tests (e2e_ap_mock)..."
if cargo test -p server --test e2e_ap_mock -- --ignored --test-threads=1 --nocapture; then
    info "  Mock AP tests: PASSED"
else
    err "Mock AP tests failed"
    kill $SERVER_PID 2>/dev/null
    exit 1
fi

# ── 10. Run Iceshrimp federation test ───────────────────────────────────────
info "Running Iceshrimp federation test (e2e_ap_iceshrimp)..."
if cargo test -p server --test e2e_ap_iceshrimp -- --ignored --test-threads=1 --nocapture; then
    info "  Iceshrimp federation test: PASSED"
else
    err "Iceshrimp federation test failed"
    kill $SERVER_PID 2>/dev/null
    exit 1
fi

# ── 11. Cleanup ─────────────────────────────────────────────────────────────
info "Stopping Emumet server..."
kill $SERVER_PID 2>/dev/null || true
wait $SERVER_PID 2>/dev/null || true
info "  Server stopped"

info "All AP E2E tests passed!"
