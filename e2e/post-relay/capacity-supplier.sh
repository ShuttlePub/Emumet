#!/usr/bin/env bash
# Physical-capacity supplier for the post-relay admission gate (T1; plan 6.6, Q1 rules).
#
# Measures the filesystem of the PostgreSQL DATA volume (the most constrained volume: data and
# WAL share it here), NOT the Emumet container, and reports it with `server capacity report`.
# Cycles start on a FIXED-RATE grid every SUPPLIER_INTERVAL_SECS (default 30; at most 30 is required: a sample older
# than 90s makes admission fail closed). A failed measurement is never replaced by invented
# healthy data: nothing is reported and freshness expires.
#
# Environment:
#   DATABASE_URL            the Emumet database that the server uses (required)
#   SERVER_CMD              the server command (default: server); e.g. a source-built binary or
#                           "docker run --rm --network <net> -e DATABASE_URL ... emumet:local"
#   SUPPLIER_INTERVAL_SECS  sampling period (default 30)
#   SUPPLIER_STATS_FILE     append "cycle <start_epoch_ms> <elapsed_ms> ok|overrun|failed" per cycle
#   SUPPLIER_ONCE=1         take one sample and exit
#   COMPOSE_PROJECT / COMPOSE_FILE / PG_SERVICE / PGDATA_PATH  where to measure (test stack)
#   SUPPLIER_MEASURE_CMD    override: a command printing "FREE_BYTES TOTAL_BYTES" for the volume
#                           (managed DB / other deployment); replaces the docker exec df probe
#   SUPPLIER_CLOCK_CMD      override: command printing RFC3339 UTC using the database clock
#                           (default: PostgreSQL clock_timestamp via the compose PostgreSQL)
set -u
: "${DATABASE_URL:?DATABASE_URL is required}"
INTERVAL="${SUPPLIER_INTERVAL_SECS:-30}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
COMPOSE_PROJECT="${COMPOSE_PROJECT:-emumet-post-relay-e2e}"
COMPOSE_FILE="${COMPOSE_FILE:-$SCRIPT_DIR/compose.yml}"
PG_SERVICE="${PG_SERVICE:-postgres}"
PGDATA_PATH="${PGDATA_PATH:-/var/lib/postgresql/data}"
read -r -a SERVER_ARGV <<<"${SERVER_CMD:-server}"

measure() {
  if [ -n "${SUPPLIER_MEASURE_CMD:-}" ]; then
    # shellcheck disable=SC2086
    $SUPPLIER_MEASURE_CMD
  else
    docker compose -p "$COMPOSE_PROJECT" -f "$COMPOSE_FILE" exec -T "$PG_SERVICE" \
      df -B1 --output=avail,size "$PGDATA_PATH" | tail -n 1
  fi
}

sample_time() {
  if [ -n "${SUPPLIER_CLOCK_CMD:-}" ]; then
    # shellcheck disable=SC2086
    $SUPPLIER_CLOCK_CMD
  else
    docker compose -p "$COMPOSE_PROJECT" -f "$COMPOSE_FILE" exec -T "$PG_SERVICE" \
      psql -U postgres -d postgres -Atc \
      "SELECT to_char(clock_timestamp() AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"')"
  fi
}

now_ms() { echo $(($(date +%s%N) / 1000000)); }

# Fixed-rate schedule: cycle N starts at first_start + N * INTERVAL, whatever the cycle cost
# (measurement + clock + report time is deducted from the sleep). A cycle that takes longer than
# the period cannot be hidden: it is written to stderr and to SUPPLIER_STATS_FILE (if set) as
#   cycle <start_epoch_ms> <elapsed_ms> ok|overrun|failed
# and the next cycle starts immediately (the missed slot is skipped, never "caught up" in a burst).
INTERVAL_MS=$((INTERVAL * 1000))
[ "${SUPPLIER_SKIP_FIRST:-0}" = 1 ] && sleep "$INTERVAL"
next_start="$(now_ms)"
while true; do
  cycle_start="$(now_ms)"
  state=ok
  # Use the validating database's clock, not the monitor host's wall clock. The timestamp is taken
  # at the start of the collection so time spent measuring/reporting consumes freshness rather
  # than renewing it.
  if now="$(sample_time 2>/dev/null)" && sample="$(measure 2>/dev/null)" && read -r free total _ <<<"$sample" \
     && [[ "${free:-}" =~ ^[0-9]+$ && "${total:-}" =~ ^[0-9]+$ && "$total" -gt 0 && "$free" -le "$total" ]]; then
    "${SERVER_ARGV[@]}" capacity report --free-bytes "$free" --total-bytes "$total" --sampled-at "$now" >/dev/null \
      || { echo "capacity report failed" >&2; state=failed; }
  else
    echo "capacity measurement failed; nothing reported (admission fails closed after 90s)" >&2
    state=failed
  fi
  end="$(now_ms)"
  elapsed=$((end - cycle_start))
  if [ "$elapsed" -gt "$INTERVAL_MS" ]; then
    state=overrun
    echo "capacity supplier cycle overran its period: ${elapsed}ms > ${INTERVAL_MS}ms" >&2
  fi
  [ -n "${SUPPLIER_STATS_FILE:-}" ] && echo "cycle $cycle_start $elapsed $state" >>"$SUPPLIER_STATS_FILE"
  [ "${SUPPLIER_ONCE:-0}" = 1 ] && exit 0
  next_start=$((next_start + INTERVAL_MS))
  if [ "$next_start" -le "$end" ]; then
    next_start="$end"
  else
    sleep "$(printf '%d.%03d' $(((next_start - end) / 1000)) $(((next_start - end) % 1000)))"
  fi
done
