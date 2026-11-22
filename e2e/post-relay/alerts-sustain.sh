#!/usr/bin/env bash
# Turns the instantaneous admission-refusal row of alerts.sql into the SUSTAINED alert of plan 6.6
# ("quota 429/503 continues"). Reads alerts.sh lines ("<severity> <alert> <value>") on stdin, passes
# them through, and adds
#   critical quota_rejection_sustained <seconds>
# once `quota_admission_refusing` has been present in every evaluation for ALERT_SUSTAIN_SECS
# (default 300 s = ten 30 s evaluations: a refusal that clears by itself within a few purge or
# delivery cycles is not an incident; Retry-After is 1 s, so senders keep retrying meanwhile). The
# alert disappears at the first evaluation without the refusing row. Evaluations must come at
# least every ALERT_SUSTAIN_GAP_SECS (default 120 s); a longer silence restarts the window rather
# than counting unobserved time.
#   ALERT_STATE_FILE  state kept between evaluations (default: ${TMPDIR:-/tmp}/post-relay-alert-sustain.state)
set -euo pipefail
SUSTAIN="${ALERT_SUSTAIN_SECS:-300}"
GAP="${ALERT_SUSTAIN_GAP_SECS:-120}"
STATE="${ALERT_STATE_FILE:-${TMPDIR:-/tmp}/post-relay-alert-sustain.state}"
now="$(date +%s)"
seen=0
while IFS= read -r line; do
  printf '%s\n' "$line"
  case "$line" in *" quota_admission_refusing "*) seen=1 ;; esac
done
if [ "$seen" = 1 ]; then
  first="$now"
  if [ -f "$STATE" ]; then
    read -r f l <"$STATE" || true
    if [[ "${f:-}" =~ ^[0-9]+$ && "${l:-}" =~ ^[0-9]+$ ]] && [ $((now - l)) -le "$GAP" ]; then
      first="$f"
    fi
  fi
  printf '%s %s\n' "$first" "$now" >"$STATE"
  if [ $((now - first)) -ge "$SUSTAIN" ]; then
    echo "critical quota_rejection_sustained $((now - first))"
  fi
else
  rm -f "$STATE"
fi
