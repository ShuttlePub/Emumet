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
#   EMUMET_E2E_PAUSE_BEFORE_CLEANUP=1 — pause before cleanup for manual inspection.
#
# Required tools:
#   - docker (compose v2 plugin)
#   - cargo
#   - openssl
#   - curl

set -euo pipefail

# ── Constants ────────────────────────────────────────────────────────────────
SERVICE_TIMEOUT=120  # Max seconds to wait per service readiness

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
command -v timeout >/dev/null 2>&1 || { err "timeout not found (required for probe timeouts)"; exit 1; }
command -v fuser >/dev/null 2>&1 || { err "fuser not found (required for port cleanup)"; exit 1; }
command -v ss >/dev/null 2>&1 || { err "ss not found (required for port checks)"; exit 1; }

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
# Allow user override via DOCKER_HOST_IP env var
export DOCKER_HOST_IP="${DOCKER_HOST_IP:-$HOST_IP}"
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

# ── 3. Generate .env if missing ────────────────────────────────────────────
if [ ! -f .env ]; then
    info "Creating .env with default configuration..."
    cat > .env <<- EOF
DATABASE_URL=postgres://postgres:develop@localhost:5432/postgres
HYDRA_ISSUER_URL=http://localhost:4444/
HYDRA_ADMIN_URL=http://localhost:4445/
KRATOS_PUBLIC_URL=http://localhost:4433/
EXPECTED_AUDIENCE=account
KETO_READ_URL=http://localhost:4466
KETO_WRITE_URL=http://localhost:4467
REDIS_URL=redis://localhost:6379
WORKER_ID=0
CORS_ALLOWED_ORIGINS=*
EOF
    info "  Created: .env with default service URLs"
fi

# ── 4. Source .env (base config) ──────────────────────────────────────────
info "Loading .env configuration..."
set -o allexport
source .env
set +o allexport

# ── 5. Build server ────────────────────────────────────────────────────────
info "Building Emumet server with test-mode features..."
cargo build -p server --features test-mode

# ── 6. Start compose infrastructure ────────────────────────────────────────
info "Starting Docker Compose infrastructure..."
info "  compose files: compose.yml + compose.ap-e2e.yml"
info "  profile: ap-e2e"

# Check port 8080 is free before starting our own server
if ss -tlnp 'sport = :8080' 2>/dev/null | grep -q LISTEN; then
    warn "Port 8080 is already in use. A stale server may be running."
    warn "Run: kill \$(lsof -t -i:8080)  or  pkill -f 'target/debug/server'"
    err "Port 8080 is occupied — aborting"
    exit 1
fi

cleanup() {
    # ── Pause before cleanup mode ──────────────────────────────────────────
    # When EMUMET_E2E_PAUSE_BEFORE_CLEANUP=1, pause before killing services
    # to allow manual inspection from the browser.
    if [ "${EMUMET_E2E_PAUSE_BEFORE_CLEANUP:-0}" = "1" ]; then
        echo ""
        warn "========================================"
        warn "  PAUSE BEFORE CLEANUP"
        warn "========================================"
        info "Infrastructure is still running. Access from your browser:"
        echo ""
        echo "  Iceshrimp:  https://iceshrimp.127.0.0.1.nip.io:8443"
        echo "  Mastodon:   https://mastodon.127.0.0.1.nip.io:8443"
        echo "  Emumet:     https://emumet.127.0.0.1.nip.io:8443"
        echo "  Peer:       https://peer.127.0.0.1.nip.io:8443"
        echo ""
        info "Iceshrimp test user credentials:"
        echo "  Password: test-pass"
        echo "  Username: iceshrimp_username=...  (see test log output above, e.g. 'Signed up on Iceshrimp')"
        echo ""
        info "Press ENTER to continue cleanup (docker compose down -v, kill server)..."
        read -r
        echo ""
    fi

    info "Cleaning up..."
    if [ -n "$SERVER_PID" ]; then
        kill $SERVER_PID 2>/dev/null || true
        wait $SERVER_PID 2>/dev/null || true
    fi
    if ss -tlnp 'sport = :8080' 2>/dev/null | grep -q LISTEN; then
        fuser -k 8080/tcp 2>/dev/null || true
        sleep 1
    fi
    $COMPOSE_CMD -f compose.yml -f compose.ap-e2e.yml --profile ap-e2e down -v
    info "Cleanup complete"
}
trap cleanup EXIT

$COMPOSE_CMD -f compose.yml -f compose.ap-e2e.yml --profile ap-e2e up -d

# ── 7. Wait for services (with timeout) ─────────────────────────────────────
info "Waiting for infrastructure services..."

wait_for_service() {
    local name="$1" max=$SERVICE_TIMEOUT cmd="$2"
    if ! timeout "$max" bash -c "
        while true; do
            if timeout 5s bash -c '$cmd' 2>/dev/null; then
                echo '    $name is ready'
                exit 0
            fi
            sleep 2
        done
    " 2>/dev/null; then
        err "$name did not become ready within ${max}s"
        exit 1
    fi
}

echo "  - postgres..."
wait_for_service "postgres" "$COMPOSE_CMD exec -T postgres pg_isready -U postgres </dev/null"

echo "  - kratos..."
wait_for_service "kratos" "curl -sf --connect-timeout 2 --max-time 5 http://localhost:4433/health/alive"

echo "  - hydra..."
wait_for_service "hydra" "curl -sf --connect-timeout 2 --max-time 5 http://localhost:4444/health/alive"

echo "  - redis..."
wait_for_service "redis" "$COMPOSE_CMD exec -T redis redis-cli ping </dev/null 2>/dev/null | grep -q PONG"

echo "  - nginx (port 8443)..."
wait_for_service "nginx" "echo >/dev/tcp/localhost/8443 2>/dev/null"

echo "  - iceshrimp..."
# Check that we get a non-502 HTTP status.  nginx returns 502 when the
# upstream (iceshrimp:3000) is not yet ready, and curl -s returns 0 even
# for the 502 HTML page.  We extract the status code and reject 502.
wait_for_service "iceshrimp" "test \"\$(curl -sk -o /dev/null -w '%{http_code}' --connect-timeout 2 --max-time 5 https://iceshrimp.127.0.0.1.nip.io:8443/api/endpoint 2>/dev/null)\" != 502"

echo "  - mastodon..."
# Mastodon's health endpoint returns HTTP 200 with body "OK" when ready.
# Check that the body is exactly "OK" (not the nginx 502 HTML page).
wait_for_service "mastodon" "curl -sk --connect-timeout 2 --max-time 5 https://mastodon.127.0.0.1.nip.io:8443/health 2>/dev/null | grep -qxF OK"

echo "  - mastodon-sidekiq..."
# Look for the sidekiq process in the container as a signal that sidekiq has
# started processing. The heartbeat file pattern used by Iceshrimp does not
# exist in Mastodon's container without a custom wrapper.
wait_for_service "mastodon-sidekiq" "$COMPOSE_CMD exec -T mastodon-sidekiq pgrep -f sidekiq 2>/dev/null"

# ── 8. Set AP E2E environment ──────────────────────────────────────────────
# These override any values from .env
export AP_TEST_ALLOWED_FETCH_HOSTS="127.0.0.1,iceshrimp.127.0.0.1.nip.io,mastodon.127.0.0.1.nip.io"
export AP_TEST_ACCEPT_INVALID_CERTS="1"
export EMUMET_E2E_EXTERNAL_SERVER="1"
# Use direct HTTP for the server API (bypasses nginx to avoid potential
# HTTPS proxy issues), while using the public HTTPS URL for ActivityPub
# signing and ID generation. This separation was found necessary because
# nginx 1.31.2 can return 400 (upstream=-) for certain HTTPS POST requests
# from reqwest (see e2e/certs/nginx.conf for details).
export EMUMET_E2E_SERVER_BASE_URL="http://localhost:8080"
export EMUMET_E2E_PUBLIC_BASE_URL="https://emumet.127.0.0.1.nip.io"
export PUBLIC_BASE_URL="https://emumet.127.0.0.1.nip.io"
export ICESHRIMP_BASE_URL="https://iceshrimp.127.0.0.1.nip.io:8443"
export MASTODON_BASE_URL="https://mastodon.127.0.0.1.nip.io:8443"
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
echo "  MASTODON_BASE_URL=$MASTODON_BASE_URL"
echo "  EMUMET_TEST_MODE_TOKEN=(set — value not logged)"

# ── 9. Start Emumet server ─────────────────────────────────────────────────
info "Starting Emumet server in test-mode (host process)..."
cargo run -p server --features test-mode &
SERVER_PID=$!
info "  Server PID: $SERVER_PID"

# Wait for server to be ready (via test-mode health endpoint on direct HTTP)
info "Waiting for Emumet server..."
for i in $(seq 1 30); do
    if curl -sf "http://localhost:8080/__test__/health" \
         -H "X-Emumet-Test-Token: ${EMUMET_TEST_MODE_TOKEN}" >/dev/null 2>&1; then
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

# ── 10. Run basic flow E2E tests ─────────────────────────────────────────────
# These test core CRUD operations through the external server.
info "Running basic flow E2E tests (e2e_basic_flow)..."
if cargo test -p server --test e2e_basic_flow -- --ignored --test-threads=1 --nocapture; then
    info "  Basic flow tests: PASSED"
else
    err "Basic flow tests failed"
    kill $SERVER_PID 2>/dev/null
    exit 1
fi

# ── 11. Run mock AP E2E tests ──────────────────────────────────────────────
info "Running mock AP E2E tests (e2e_ap_mock)..."
if cargo test -p server --test e2e_ap_mock -- --ignored --test-threads=1 --nocapture; then
    info "  Mock AP tests: PASSED"
else
    err "Mock AP tests failed"
    kill $SERVER_PID 2>/dev/null
    exit 1
fi

# ── 12. Run Iceshrimp federation test ───────────────────────────────────────
info "Running Iceshrimp federation test (e2e_ap_iceshrimp)..."
if cargo test -p server --test e2e_ap_iceshrimp -- --ignored --test-threads=1 --nocapture; then
    info "  Iceshrimp federation test: PASSED"
else
    err "Iceshrimp federation test failed"
    kill $SERVER_PID 2>/dev/null
    exit 1
fi

# ── 13. Run Mastodon federation test ────────────────────────────────────────
info "Running Mastodon federation test (e2e_ap_mastodon)..."
if cargo test -p server --test e2e_ap_mastodon -- --ignored --test-threads=1 --nocapture; then
    info "  Mastodon federation test: PASSED"
else
    err "Mastodon federation test failed"
    kill $SERVER_PID 2>/dev/null
    exit 1
fi

# ── 14. Cleanup ─────────────────────────────────────────────────────────────
info "Stopping Emumet server..."
kill $SERVER_PID 2>/dev/null || true
wait $SERVER_PID 2>/dev/null || true
info "  Server stopped"

info "All AP E2E tests passed!"
