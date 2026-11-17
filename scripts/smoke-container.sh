#!/usr/bin/env bash
# Smoke test of an Emumet server image. The same script runs locally and in CI.
#
#   scripts/smoke-container.sh <image-ref>
#
# Environment:
#   CONTAINER_ENGINE      docker or podman (default: docker if present, else podman)
#   SMOKE_POSTGRES_IMAGE  override the pinned Postgres image (default below)
#
# It needs the engine CLI and curl on the host. The image itself needs no curl: health checks are
# done from the host through a published loopback port.
#
# Everything it creates (one network, one Postgres container, one key volume, the server
# containers) carries a unique per-run suffix, and only those are removed on exit, whether the run
# succeeds, fails or is interrupted. It never prunes and never touches anything else. Secret
# content (the dummy master key, passwords) is never printed.
#
# What it proves, and what it does not:
#   * the image starts as UID/GID 10001 on a read-only root filesystem with no log mount,
#     contains only /app/server (no compiler, no cargo, no curl) and is a non-test-mode build;
#   * a 0400 master key owned by UID 10001 can be read by the default user at OS level.
#     It does NOT prove that the application derives or uses the key correctly;
#   * migrations apply to an empty database: the _sqlx_migrations count equals the number of SQL
#     files in migrations/;
#   * GET /healthz is 200 with the exact body "ok\n", also while the database is down;
#   * SIGTERM (docker stop -t 30) and SIGINT stop the server with exit code 0; a stop request that
#     arrives while the migration is blocked is honoured as soon as it finishes;
#   * EMUMET_FILE_LOG_ENABLED: unset writes no file, `true` writes ./logs, other values fail.
set -euo pipefail

if [ "$#" -ne 1 ] || [ -z "$1" ]; then
  echo "usage: $0 <image-ref>" >&2
  exit 2
fi
IMAGE="$1"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

POSTGRES_IMAGE="${SMOKE_POSTGRES_IMAGE:-postgres:16@sha256:1a6ab3f5345eb6dbe04a1349529caabdb0ab09293a09590fad07b2246bfa4b54}"
RUN_USER=10001
STOP_GRACE=30
PG_PASSWORD=smoke-db-password
DB_NAME=emumet

if [ -n "${CONTAINER_ENGINE:-}" ]; then
  ENGINE="$CONTAINER_ENGINE"
elif command -v docker >/dev/null 2>&1; then
  ENGINE=docker
elif command -v podman >/dev/null 2>&1; then
  ENGINE=podman
else
  echo "neither docker nor podman is available" >&2
  exit 2
fi
command -v curl >/dev/null 2>&1 || { echo "curl is required on the host" >&2; exit 2; }

SUFFIX="$(date +%s)-$$-${RANDOM}"
NETWORK="emumet-smoke-net-${SUFFIX}"
KEY_VOLUME="emumet-smoke-key-${SUFFIX}"
PG="emumet-smoke-pg-${SUFFIX}"
WORKDIR_TMP="$(mktemp -d)"
CONTAINERS=()
KEY_CREATED=0
NETWORK_CREATED=0
FAILED=0

log() { printf '[smoke] %s\n' "$*"; }
die() { printf '[smoke] FAIL: %s\n' "$*" >&2; FAILED=1; exit 1; }

dump_logs() {
  local c
  for c in "${CONTAINERS[@]}"; do
    if "$ENGINE" inspect "$c" >/dev/null 2>&1; then
      printf '[smoke] ---- last logs of %s ----\n' "$c" >&2
      "$ENGINE" logs --tail 60 "$c" >&2 2>&1 || true
    fi
  done
}

cleanup() {
  local status=$?
  set +e
  if [ "$status" -ne 0 ] || [ "$FAILED" -ne 0 ]; then dump_logs; fi
  local c
  for c in "${CONTAINERS[@]}"; do
    "$ENGINE" rm -f "$c" >/dev/null 2>&1
  done
  if [ "$KEY_CREATED" -eq 1 ]; then "$ENGINE" volume rm -f "$KEY_VOLUME" >/dev/null 2>&1; fi
  if [ "$NETWORK_CREATED" -eq 1 ]; then "$ENGINE" network rm "$NETWORK" >/dev/null 2>&1; fi
  rm -rf "$WORKDIR_TMP"
  if [ "$status" -eq 0 ]; then log "cleaned up (suffix ${SUFFIX})"; fi
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# ---- helpers ----------------------------------------------------------------------------------

# start_server <label> [engine run options...]: starts a detached server container and stores its
# name in $SERVER. Common options: dedicated network, read-only root, no capabilities, the key
# volume mounted read-only, only DATABASE_URL set (every other setting has a default).
# $PORT is the container port to publish on a random loopback port.
SERVER=""
PORT=8080
start_server() {
  local label="$1"; shift
  SERVER="emumet-smoke-${label}-${SUFFIX}"
  CONTAINERS+=("$SERVER")
  "$ENGINE" run -d --name "$SERVER" --network "$NETWORK" \
    --read-only --cap-drop ALL --security-opt no-new-privileges \
    -v "$KEY_VOLUME:/run/secrets:ro" \
    -e "DATABASE_URL=postgres://postgres:${PG_PASSWORD}@${PG}:5432/${DB_NAME}" \
    -p "127.0.0.1::${PORT}" \
    "$@" "$IMAGE" >/dev/null
}

container_state() { "$ENGINE" inspect -f '{{.State.Status}}' "$1"; }
exit_code() { "$ENGINE" inspect -f '{{.State.ExitCode}}' "$1"; }

host_port() {
  "$ENGINE" port "$1" "${2}/tcp" | head -n 1 | sed 's/.*://'
}

# wait_for_exit <container> <seconds>
wait_for_exit() {
  local c="$1" limit="$2" i=0
  while [ "$i" -lt "$limit" ]; do
    if [ "$(container_state "$c")" != "running" ]; then return 0; fi
    sleep 1
    i=$((i + 1))
  done
  die "$c did not exit within ${limit}s"
}

# wait_healthy <container> <container-port>: waits until GET /healthz is 200; the container must
# stay running. Sets HOST_PORT.
HOST_PORT=""
wait_healthy() {
  local c="$1" port="$2" i=0 code
  while [ "$i" -lt 90 ]; do
    if [ "$(container_state "$c")" != "running" ]; then
      die "$c stopped before becoming healthy (exit code $(exit_code "$c"))"
    fi
    HOST_PORT="$(host_port "$c" "$port" 2>/dev/null || true)"
    if [ -n "$HOST_PORT" ]; then
      code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 2 "http://127.0.0.1:${HOST_PORT}/healthz" || true)"
      if [ "$code" = "200" ]; then return 0; fi
    fi
    sleep 1
    i=$((i + 1))
  done
  die "$c did not answer GET /healthz with 200 within 90s"
}

http_code() { curl -s -o /dev/null -w '%{http_code}' --max-time 5 "$1" || true; }

# pg_sql <sql>: runs a query as the superuser and prints the bare result.
pg_sql() { "$ENGINE" exec "$PG" psql -U postgres -d "$DB_NAME" -tA -c "$1"; }

wait_pg_ready() {
  local i=0
  while [ "$i" -lt 60 ]; do
    # pg_isready alone also succeeds against the temporary init-time server; require a real query.
    if "$ENGINE" exec "$PG" psql -U postgres -d "$DB_NAME" -tA -c 'select 1' >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
    i=$((i + 1))
  done
  die "postgres did not become ready within 60s"
}

logs_of() { "$ENGINE" logs "$1" 2>&1; }

assert_logs_contain() {
  logs_of "$1" > "$WORKDIR_TMP/assert.txt"
  grep -Fq -- "$2" "$WORKDIR_TMP/assert.txt" || die "$1: expected log text not found: $2"
}

assert_no_secret_in_logs() {
  logs_of "$1" > "$WORKDIR_TMP/logs.txt"
  if grep -Fq -f "$WORKDIR_TMP/key-content" "$WORKDIR_TMP/logs.txt"; then
    die "$1: the master key content appears in the logs"
  fi
  if grep -Fq -- "$PG_PASSWORD" "$WORKDIR_TMP/logs.txt"; then
    die "$1: the database password appears in the logs"
  fi
}

# ---- set-up -----------------------------------------------------------------------------------
log "engine: $ENGINE ($("$ENGINE" --version | head -n 1))"
log "image:  $IMAGE"
log "suffix: $SUFFIX"

"$ENGINE" image inspect "$IMAGE" >/dev/null 2>&1 || die "image not found locally: $IMAGE"
if ! "$ENGINE" image inspect "$POSTGRES_IMAGE" >/dev/null 2>&1; then
  "$ENGINE" pull -q "$POSTGRES_IMAGE" >/dev/null
fi

"$ENGINE" network create "$NETWORK" >/dev/null
NETWORK_CREATED=1

# N5: the key lives in a named volume that a helper container fills, so that the owner is UID
# 10001 and the mode is 0400 without sudo and without host-UID bind-mount problems. The volume is
# then mounted read-only. The content is random and is only ever compared, never printed.
"$ENGINE" volume create "$KEY_VOLUME" >/dev/null
KEY_CREATED=1
head -c 24 /dev/urandom | od -An -tx1 | tr -d ' \n' > "$WORKDIR_TMP/key-content"
"$ENGINE" run --rm -i -v "$KEY_VOLUME:/k" --entrypoint sh "$POSTGRES_IMAGE" -c '
  set -e
  umask 077
  cat > /k/master-key-password
  chown '"$RUN_USER:$RUN_USER"' /k/master-key-password
  chmod 0400 /k/master-key-password
  chmod 0755 /k' < "$WORKDIR_TMP/key-content"

CONTAINERS+=("$PG")
"$ENGINE" run -d --name "$PG" --network "$NETWORK" \
  -e POSTGRES_PASSWORD="$PG_PASSWORD" -e POSTGRES_DB="$DB_NAME" \
  "$POSTGRES_IMAGE" >/dev/null
wait_pg_ready
log "postgres is ready"

# ---- A. image contents and the key, checked as the image's default user -----------------------
log "A. image contents, user and master key"
REVISION_LABEL="$("$ENGINE" image inspect -f '{{index .Config.Labels "org.opencontainers.image.revision"}}' "$IMAGE")"
[ -n "$REVISION_LABEL" ] || die "the image has no org.opencontainers.image.revision label"
[ "$("$ENGINE" image inspect -f '{{.Architecture}}' "$IMAGE")" = "amd64" ] || die "the image is not amd64"
[ "$("$ENGINE" image inspect -f '{{.Config.User}}' "$IMAGE")" = "10001:10001" ] || die "the image user is not 10001:10001"
[ "$("$ENGINE" image inspect -f '{{.Config.StopSignal}}' "$IMAGE")" = "SIGTERM" ] || die "STOPSIGNAL is not SIGTERM"

# shellcheck disable=SC2016  # the script runs inside the container
"$ENGINE" run --rm --read-only --cap-drop ALL --security-opt no-new-privileges \
  -v "$KEY_VOLUME:/run/secrets:ro" --entrypoint sh "$IMAGE" -c '
  set -eu
  [ "$(id -u)" = 10001 ] && [ "$(id -g)" = 10001 ] || { echo "not running as 10001:10001"; exit 1; }
  [ "$(stat -c "%u:%g %a" /run/secrets/master-key-password)" = "10001:10001 400" ] \
    || { echo "unexpected owner/mode of the key"; exit 1; }
  [ -s /run/secrets/master-key-password ] || { echo "the key is empty"; exit 1; }
  # a real read of the whole file by the default user (content is discarded, never printed)
  cat /run/secrets/master-key-password > /dev/null
  [ "$(ls -A /app)" = "server" ] || { echo "/app holds more than the server binary"; exit 1; }
  for tool in cargo rustc gcc cc make curl wget; do
    if command -v "$tool" >/dev/null 2>&1; then echo "forbidden tool in the image: $tool"; exit 1; fi
  done
  if [ -e /src ] || [ -e /migrations ] || [ -e /app/migrations ]; then
    echo "sources or migrations in the image"; exit 1
  fi
  if ldd /app/server | grep "not found"; then echo "missing shared libraries"; exit 1; fi' \
  || die "image content / key checks failed"
log "A. ok (user 10001:10001, key 0400 readable, only /app/server, no build tools, no missing libraries)"

# ---- B. normal start: health, test-mode, migrations, DB outage, docker stop -------------------
log "B. default start on a read-only root without a log mount"
PORT=8080
start_server main
MAIN="$SERVER"
wait_healthy "$MAIN" 8080

HEALTHZ_CODE="$(curl -s -o "$WORKDIR_TMP/healthz.body" -w '%{http_code}' --max-time 5 "http://127.0.0.1:${HOST_PORT}/healthz")"
[ "$HEALTHZ_CODE" = "200" ] || die "/healthz is $HEALTHZ_CODE, not 200"
printf 'ok\n' > "$WORKDIR_TMP/healthz.expected"
cmp -s "$WORKDIR_TMP/healthz.body" "$WORKDIR_TMP/healthz.expected" || die "/healthz body is not exactly 'ok\\n'"
[ "$(http_code "http://127.0.0.1:${HOST_PORT}/__test__/health")" = "404" ] || die "/__test__/health is not 404 (test-mode build?)"
[ "$(http_code "http://127.0.0.1:${HOST_PORT}/api/v1/me")" = "401" ] || die "/api/v1/me without a token is not 401"

EXPECTED_MIGRATIONS="$(find "$REPO_ROOT/migrations" -maxdepth 1 -name '*.sql' | wc -l | tr -d ' ')"
APPLIED_MIGRATIONS="$(pg_sql 'select count(*) from _sqlx_migrations where success')"
ALL_MIGRATIONS="$(pg_sql 'select count(*) from _sqlx_migrations')"
[ "$EXPECTED_MIGRATIONS" -gt 0 ] || die "no migrations found in $REPO_ROOT/migrations"
if [ "$APPLIED_MIGRATIONS" != "$EXPECTED_MIGRATIONS" ] || [ "$ALL_MIGRATIONS" != "$EXPECTED_MIGRATIONS" ]; then
  die "_sqlx_migrations has $APPLIED_MIGRATIONS successful of $ALL_MIGRATIONS rows, migrations/ has $EXPECTED_MIGRATIONS files"
fi
log "B. migrations applied: $APPLIED_MIGRATIONS = $EXPECTED_MIGRATIONS files"

assert_logs_contain "$MAIN" "Snowflake ID generator initialized"
assert_no_secret_in_logs "$MAIN"

# DB outage: /healthz only means "the HTTP server is up" and must not touch the database.
"$ENGINE" stop -t 10 "$PG" >/dev/null
[ "$(http_code "http://127.0.0.1:${HOST_PORT}/healthz")" = "200" ] || die "/healthz depends on the database"
"$ENGINE" start "$PG" >/dev/null
wait_pg_ready
log "B. /healthz stays 200 while the database is stopped"

STOP_STARTED="$(date +%s)"
"$ENGINE" stop -t "$STOP_GRACE" "$MAIN" >/dev/null
STOP_SECONDS=$(( $(date +%s) - STOP_STARTED ))
[ "$(exit_code "$MAIN")" = "0" ] || die "docker stop -t $STOP_GRACE ended with exit code $(exit_code "$MAIN")"
assert_logs_contain "$MAIN" "Stop requested; shutting down"
log "B. stop -t $STOP_GRACE: exit code 0 after ${STOP_SECONDS}s"

# ---- C. direct signals, non-default port ------------------------------------------------------
for signal in INT TERM; do
  log "C. SIG${signal} on a non-default port"
  PORT=18080
  start_server "sig-$(printf '%s' "$signal" | tr '[:upper:]' '[:lower:]')" -e HTTP_LISTEN_ADDR=0.0.0.0:18080
  wait_healthy "$SERVER" 18080
  "$ENGINE" kill --signal="$signal" "$SERVER" >/dev/null
  wait_for_exit "$SERVER" 30
  [ "$(exit_code "$SERVER")" = "0" ] || die "SIG${signal} ended with exit code $(exit_code "$SERVER")"
  assert_logs_contain "$SERVER" "Stop requested; shutting down"
done
PORT=8080
log "C. SIGINT and SIGTERM both exit 0 through the same path"

# ---- E. a stop request while the migration is blocked -----------------------------------------
log "E. stop request during the (uninterruptible) migration step"
# Hold an ACCESS EXCLUSIVE lock on the migrations table: the migrator blocks on its first read.
"$ENGINE" exec "$PG" psql -U postgres -d "$DB_NAME" -c \
  'begin; lock table _sqlx_migrations in access exclusive mode; select pg_sleep(12); commit;' \
  >/dev/null 2>&1 &
LOCK_PID=$!
sleep 2
start_server blocked
BLOCKED="$SERVER"
sleep 4
[ "$(container_state "$BLOCKED")" = "running" ] || die "the server did not wait for the migration lock"
"$ENGINE" kill --signal=TERM "$BLOCKED" >/dev/null
wait_for_exit "$BLOCKED" 40
wait "$LOCK_PID" || true
[ "$(exit_code "$BLOCKED")" = "0" ] || die "a stop request during the migration ended with exit code $(exit_code "$BLOCKED")"
assert_logs_contain "$BLOCKED" "Stop requested during start-up"
log "E. the deferred stop request was honoured right after the migration step (exit 0)"

# ---- F. an unreachable database is a start-up failure -----------------------------------------
log "F. unreachable database"
NODB="emumet-smoke-nodb-${SUFFIX}"
CONTAINERS+=("$NODB")
"$ENGINE" run -d --name "$NODB" --network "$NETWORK" --read-only --cap-drop ALL \
  -v "$KEY_VOLUME:/run/secrets:ro" \
  -e "DATABASE_URL=postgres://postgres:${PG_PASSWORD}@emumet-smoke-no-such-host-${SUFFIX}:5432/${DB_NAME}" \
  "$IMAGE" >/dev/null
wait_for_exit "$NODB" 60
[ "$(exit_code "$NODB")" != "0" ] || die "the server exited 0 although the database is unreachable"
log "F. exit code $(exit_code "$NODB") (non-zero as required)"

# ---- G. file log switch -----------------------------------------------------------------------
for bad in TRUE 1 ""; do
  log "G. EMUMET_FILE_LOG_ENABLED='${bad}' must be a start-up error"
  start_server badlog -e "EMUMET_FILE_LOG_ENABLED=${bad}"
  wait_for_exit "$SERVER" 30
  [ "$(exit_code "$SERVER")" != "0" ] || die "EMUMET_FILE_LOG_ENABLED='${bad}' was accepted"
  "$ENGINE" rm -f "$SERVER" >/dev/null
done
log "G. true on a read-only root without a log mount must fail (never silently disabled)"
start_server lognomount -e EMUMET_FILE_LOG_ENABLED=true
wait_for_exit "$SERVER" 30
[ "$(exit_code "$SERVER")" != "0" ] || die "EMUMET_FILE_LOG_ENABLED=true started although ./logs is not writable"
"$ENGINE" rm -f "$SERVER" >/dev/null

log "G. true with a writable log mount writes ./logs/debug.log.<date> and still logs to stdout"
start_server logfile -e EMUMET_FILE_LOG_ENABLED=true \
  --tmpfs "/app/logs:rw,uid=${RUN_USER},gid=${RUN_USER},mode=0755"
wait_healthy "$SERVER" 8080
LOGFILES="$("$ENGINE" exec "$SERVER" ls /app/logs 2>/dev/null || true)"
case "$LOGFILES" in
  debug.log.*) ;;
  *) die "no ./logs/debug.log.<date> was written" ;;
esac
assert_logs_contain "$SERVER" "Snowflake ID generator initialized"
"$ENGINE" stop -t "$STOP_GRACE" "$SERVER" >/dev/null
[ "$(exit_code "$SERVER")" = "0" ] || die "the file-log run ended with exit code $(exit_code "$SERVER")"

log "ALL SMOKE CHECKS PASSED ($IMAGE; revision label ${REVISION_LABEL})"
