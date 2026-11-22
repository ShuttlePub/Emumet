#!/usr/bin/env bash
# Evaluates the post-relay alert conditions once and prints one line per firing alert:
#   <severity> <alert> <value>
# Exit status 0 even when alerts fire (the monitor decides); non-zero if ANY part of the
# evaluation failed (alerts.sql, the reservation check or its statement file, the sustain filter):
# a missing evaluation is never reported as "nothing firing".
# Needs psql and DATABASE_URL. Run it every 30 s next to capacity-supplier.sh.
set -euo pipefail
: "${DATABASE_URL:?DATABASE_URL is required}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONSISTENCY="${CONSISTENCY_SQL:-$HERE/../../driver/src/database/postgres/post_relay/consistency.sql}"
[ -f "$CONSISTENCY" ] || { echo "consistency statement not found: $CONSISTENCY" >&2; exit 2; }
rows="$(psql "$DATABASE_URL" -X -A -t -F ' ' -v ON_ERROR_STOP=1 -f "$HERE/alerts.sql")"
ok="$(psql "$DATABASE_URL" -X -A -t -v ON_ERROR_STOP=1 -f "$CONSISTENCY")"
{
  if [ -n "$rows" ]; then printf '%s\n' "$rows" | awk 'NF { print $2, $1, $3 }'; fi
  if [ "$ok" != t ]; then echo "critical reservation_mismatch 1"; fi
} | "$HERE/alerts-sustain.sh"
