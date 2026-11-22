#!/usr/bin/env bash
# Sustained Relay-admission refusals (plan 6.6: "quota 429/503 continues"), including the DB-shared
# rate limit buckets, which leave no database trace. Reads the Emumet process log on stdin (a tail
# of `docker logs` / journald that covers at least REFUSAL_SUSTAIN_TICKS+1 telemetry lines) and
# uses the cumulative counters of the periodic line
#   post relay inbox refusals global_rate=N profile_rate=N profile_quota=N global_quota=N
#                              quarantine_full=N capacity=N storage=N
# (fixed reason codes, no identifiers; period POST_RELAY_TELEMETRY_SECS, default 60 s).
# Rule: the sum of the quota/capacity reasons (everything except `storage`) ROSE in each of the last
# REFUSAL_SUSTAIN_TICKS consecutive telemetry intervals (default 5 = about 5 minutes). It clears at
# the first interval without a refusal. A counter that went down means a process restart: that
# interval restarts the streak. Prints, when firing:
#   critical quota_refusals_sustained <intervals> <reasons that rose in the latest interval>
set -euo pipefail
N="${REFUSAL_SUSTAIN_TICKS:-5}"
sed 's/\x1b\[[0-9;]*m//g' | awk -v need="$N" '
  /post relay inbox refusals/ {
    split("global_rate profile_rate profile_quota global_quota quarantine_full capacity", keys, " ")
    rose = ""; fell = 0; have = 1
    for (i = 1; i <= 6; i++) {
      v = -1
      if (match($0, keys[i] "=[0-9]+")) v = substr($0, RSTART + length(keys[i]) + 1, RLENGTH - length(keys[i]) - 1) + 0
      if (v < 0) { have = 0; break }
      cur[i] = v
      if (seen) { if (v > prev[i]) rose = rose (rose == "" ? "" : ",") keys[i]; if (v < prev[i]) fell = 1 }
    }
    if (!have) next
    if (seen) {
      if (fell) { streak = 0; last = "" }
      else if (rose != "") { streak++; last = rose }
      else { streak = 0; last = "" }
    }
    for (i = 1; i <= 6; i++) prev[i] = cur[i]
    seen = 1
  }
  END { if (streak >= need) print "critical quota_refusals_sustained " streak " " last }
'
