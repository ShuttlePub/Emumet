# Post-relay sending and recovery (P1 S1 / B1)

The queue is separate from Profile federation delivery. S1 does not enable inbox forwarding
(I1), change the relay contract, rotate keys (K2), or add a management HTTP API.
[Storage/capacity operations](post-relay-storage.md), [links](home-links.md),
[instance identity](instance-identity.md), [measurements](post-relay-measurements.md).

## Worker and safety boundary

Normal no-argument server startup (including cargo run -p server) is unchanged. After
identity provisioning it starts a bounded relay pool: at most 16 local tasks; PostgreSQL
permit slots enforce **16 globally and 4 per origin across processes and manual commands**.
The process admits at most four relay DB phases concurrently; normal API/projector pool
headroom remains necessary. This is not permission to enlarge the deployment's DB pool.
Scheduler rows rotate recently served Profiles and interleave origins; near-deadline jobs
get priority, saturated origins are excluded before claiming. Maintenance has its own
one-second turn and bounded per-Profile transactions, including while HTTP tasks are full.

Sending starts at the **acknowledged authorization commit**, not claim/signature preparation.
An unknown commit result never sends. The DB permit fixes attempt/fence/link/generation,
authorized_at, send_deadline, and a shared slot. Absolute deadline is min(authorized_at+10s,
received_at+72h), or min(authorized_at+10s, original quarantine expiry) for manual delivery.
A monotonic marker taken before the permission DB call conservatively includes pool/lock/
commit latency; it can shorten but never restart/enlarge the budget. DNS/connect/TLS/send/
response and post-authorization scheduling all consume the same budget. Results obtained
after it are not reported to storage as in-time observations. An in-time result may be
persisted later while its original fenced lease remains live; a stale lease cannot restore
body/quota/job state.

HTTP holds no DB connection, transaction, row/advisory lock. Unlink, Account deactivation and
projector do not wait for it. A permit committed before unlink may still deliver within its
original deadline; already forwarded bytes cannot be recalled. Crash/restart does not reuse
a permit; 30s leases and fencing allow recovery after its original deadline. A manual attempt
is at most once per explicit operation; a crash after authorization may therefore require
another explicit operation after recovery. No exactly-once delivery guarantee is made.

Transport: HTTPS, no redirect/proxy, freshly validated DNS, reject any internal/mixed address
set, and connect only to the validated addresses. The bounded client cache (64 entries,
60s idle) is keyed by **origin and the exact freshly validated address set**; DNS is still
rechecked every attempt. It stores no body/key and is not a DNS approval cache. Insecure
loopback construction exists only in cfg(test)/test-mode and is absent from release builds.
Stored bytes are not reserialized. Only outer Date/Cavage instance signature is regenerated;
(request-target), host, date, digest are signed using RSA-2048/SHA-256. K1's parsed-key cache
(maximum 2, TTL 15m, single-flight miss) avoids per-delivery KDF and PEM parsing.

2xx succeeds; 401/408/429/5xx, DNS/network/timeout/transient signing failure retry. Other 4xx,
3xx and invalid/SSRF targets fail permanently. Retry starts at 30s exponential base, capped
1h, equal jitter 50–100%; valid 429/503 Retry-After is a floor capped by 1h/remaining TTL.
Configuration removal or suspended/banned owner is a hold, never a fresh 72h budget.

## Sender-only stop and emergency stop

Set POST_RELAY_SENDER_ENABLED=false and restart **every sending server process**. Only the
post-relay sending stops; post-relay expiry/purge maintenance continues every second.
The normal listener/inbox/API/Profile delivery remain configured as
before. Manual redeliver commands also refuse this setting. true/unset enables, any other
value is a configuration error. Existing process configuration does not change in place.
In-flight authorized sends drain for a bounded period; disabling is not instantaneous
revocation. Close operator sessions too. If immediate egress prevention is needed, apply
deployment/network egress blocking or stop all sending processes (stopping the whole server
also stops inbox). Neither can retract bytes already accepted by a peer.

Removing an origin from the allowlist alone is **not** an emergency revocation: rolling
processes can have old configuration, and authorized attempts may already be in flight.
The owner-only unlink API is not an operator API; ask the current owner to unlink if suitable.
Continued reception while sending is stopped consumes the common job/quarantine quota;
watch backlog, oldest receipt/deadline, quota and physical capacity. No early automatic
quarantine discard is permitted to make room for new messages.

## Minimal shipped command surface

    server --help
    server post-relay --help
    server post-relay status --operation OPERATION_UUID

The release image's entrypoint is /app/server: append these arguments to docker run. Commands
need neither source nor Cargo. They bypass AppModule, Ory/JWKS, S3, normal listener, and
normal workers. They **never apply migrations**; the exact embedded migration versions,
success markers and checksums must match. Use the matching deployed image, not a different
version to upgrade the DB. DATABASE_URL is required. Errors identify a fixed, sanitized
phase (schema, configuration, signing key, operation transaction or sending), never raw
SQL/key/path details. Do not put credentials into logs.

Only operator UUIDs and PostgreSQL fence/attempt sequences generate IDs here. No Snowflake
initialization/generation occurs: WORKER_ID is not used and cannot collide with the running
server. Normal server WORKER_ID requirements remain unchanged. Redelivery additionally needs
PUBLIC_BASE_URL matching the provisioned identity, its master password secret (read-only
/run/secrets/master-key-password or chmod 600 ./master-key-password), and allowed origins.
Only existing keys are unlocked; operator commands never provision/repair an identity.

### Inspect → confirmed bounded action → inspect result

Use a fresh UUID per explicit operation. Profile ID is the immutable numeric ID, not nanoid.
Supply --receipt RECEIPT for one receipt; omit it for a Profile snapshot. Operator and reason
are required, bounded audit metadata (no secrets/payloads). Preview works while sending is off.

    server post-relay preview --action redeliver --profile-id PROFILE_ID \
      --operation OPERATION_UUID --operator OPERATOR --reason 'incident recovery'

Output includes quarantined state, count, logical bytes, earliest expiry, max_batch=128 and
a COUNT:BYTES:BOUNDARY confirmation token. It does not contain bodies. For explicit discard,
preview with --action discard instead; **discard is separate, irreversible, and never automatic**.
Review the target, bytes, expiry and current link/capacity before continuing.

    server post-relay redeliver --profile-id PROFILE_ID \
      --operation OPERATION_UUID --operator OPERATOR --reason 'incident recovery' \
      --confirm COUNT:BYTES:BOUNDARY

Or use discard with the same target/operator/reason/confirmation flags. Missing, stale or
wrong confirmation rolls the transaction back. At first execution the operation freezes the
Profile/link/generation, receipt filter, sequence boundary and target count/bytes; new failures
are not followed. Each invocation stages/discards **at most 128** from that selection. Repeat
with the same UUID/flags/**original confirmation** to advance its bounded cursor. Intervening
competing actions are skipped/audited; do not silently broaden the selection with a new UUID.
Status reports original selection, staged/success/failed/discarded/skipped/cancelled/pending
counts and whether selection staging is complete. A completed selection is not necessarily
zero pending sends. Interrupted staging can resume; interrupted authorized manual delivery
is recovered without automatically repeating that authorized attempt.

Redelivery preflights keys, current owner/link/generation and allowlist before staging; every
permit rechecks eligibility again under the shared gates. It uses the same shared slots,
immutable envelope, receipt and destination as automatic sending. The CLI drains only its own
staged operation, while normal workers may compete for the same fenced jobs. It waits at most
400s per invocation, then reports failure; any committed staged jobs remain visible/recoverable
in status. Check status before deciding to resume or create another explicit operation.

Success deletes the body; failure returns to quarantine with **unchanged original expiry**.
Automatic 72h and quarantine 30d are never reset; no body edit, destination reassignment or
old-link revival is possible. Unlink/deactivation cancels both job and quarantine use, with
bounded physical purge. Discard cannot revoke previously authorized bytes. Protected operation
capacity is reserved separately; failure to reserve audit capacity refuses the operation.
Operator/batch/link/selection/time/reason/count/result audit is retained 30 days, body-free.
Success/cancellation leave no per-delivery terminal history. Permanent failure and 72h cutoff
keep unchanged quarantine plus one bounded body-free summary, 30 days from the **logical**
terminal time; delayed cleanup does not extend retention. Do not use quarantine for search or
normal content storage. Treat backups/restores according to storage retention/access policy.

## Monitoring and recovery order

Capacity samples must come from the **most constrained DB data/WAL/tablespace volume** or
managed-DB metric, not the server container disk. Use server capacity report at least every
30s; >90s-old/unknown samples fail closed. Admission stops below 10 GiB OR 10%; recovery
requires 12 GiB AND 12%. See post-relay-storage.md for the complete command and ownership.

Alert: body/audit quota 80% warning, 95% critical; sustained quota429/503; missing/stale physical
capacity; reservation mismatch; 401 streaks; near-deadline backlog; purge lag; DB lock timeouts,
connection wait and projector failures. Logs have fixed status labels/timing fields only—no
body, response body, credentials, arbitrary receipt/Activity IDs or signing material. The
sender emits cumulative DB-phase/DB-slot-wait/signature/HTTP/authorization-to-HTTP timings and
attempt/key-lookup counts each minute; key lookups are **not** KDF runs. Metrics are process
local; storage counters/slots/quotas are shared. DB-phase time includes connection/lock waits,
not an isolated lock-wait distribution. Inspect pg_stat_activity/pg_locks alongside it.

Recovery: stop/diagnose faulty destination → check Profile quota/earliest expiry and physical
sample → preview bounded redelivery → confirm → inspect status/counters/remaining quota.
If recovery is explicitly abandoned, perform a separate preview/confirmed discard. Capacity
pressure alone never authorizes discard. A full protected quarantine may block new receipts
for up to 30 days until recovery, explicit discard or expiry. Do not change that policy by
raising limits or shortening retention. Restored DBs must purge expired/unlinked bodies and
reconcile link/capacity state before sending resumes.

Throughput/latency are measured targets, not production SLOs. See the environment and outstanding
measurements in post-relay-measurements.md. Approximate catch-up time is backlog/(measured
send capacity - continuing arrival rate), only if the denominator is positive and deadlines
can still be met. Report unmet targets to the lead before I1 activation.

## Ory-independent runtime (B1)

Startup applies the existing migrations and checks/provisions Profile and instance keys first.
Key corruption, password failure and public-origin mismatch remain fatal; Ory is **not** a
fallback for these dependencies. The listener is bound before starting the relay pool (a bind
failure cannot leave a sender running). The root router then serves the instance document,
HTTP-signature-guarded inbox and ordinary routes alongside the sender and retention worker.
Neither reception, its queue transaction, nor relay authorization/HTTP delivery calls Ory.
JWKS warm-up runs separately and is never a listener/inbox readiness condition. Connection
refusal and an Ory peer that accepts but stops responding therefore do not stop cold start.
Login/consent routes still call the same A1 use cases/clients; their URLs, cookies, scope,
redirects and error handling have not changed. They are not promised usable during an Ory outage.

* `JWKS_TIMEOUT_MS`: default **3000**, integer **1..60000**. One acquisition deadline covers
  refresh-mutex wait, discovery, JWKS connect/headers **and bodies** together. The HTTP client
  also bounds connect/request waits. Unknown-kid refreshes with a populated cache retain the
  existing **300-second** minimum interval. An absent/empty cache retries lazily after at least
  **1 second**, so a failed cold-start warm-up cannot lock out authentication for five minutes
  after Ory recovers. Concurrent refreshes remain single-flight and deadline-bounded. There is
  no persistent/disk JWKS cache or background retry loop.
* JWT-protected routes remain **401/fail-closed** without a usable matching key. Cached matching
  keys continue validating RS256 signatures, issuer, audience and expiry without Ory I/O.
  Failed refresh never replaces known keys with a bypass, and unknown kids are still denied.
* `SERVER_SHUTDOWN_TIMEOUT_SECS`: default **25**, integer **1..300**. On SIGTERM/SIGINT, new
  relay claims stop immediately, HTTP stops accepting connections, warm-up is cancelled, and
  worker stop signals are sent **before** waiting for HTTP requests. A blocked maintenance
  transaction is cancelled on stop too, so it cannot delay pending-claim cancellation/drain.
  HTTP, S1 relay drain
  (at most **12s**) and webhook drain (at most **20s**) run concurrently within this one overall
  deadline. An incomplete inbound body or unresponsive outbound peer cannot extend it. After
  it, the runtime cancels remaining async work and logs a fixed deadline-exceeded warning.
  Set the deployment termination grace **above** this configured bound, with scheduling margin.

Signals received during migrations/key provisioning remain latched and are honored before
serving; these existing local initialization transactions are not cancelled halfway through.
The serving-phase drain bound is not a new time limit on migrations or key generation.
A hard kill or drain cutoff can leave committed work with its original **30s** lease. Restart
never reuses an old permit or resets the **72h** automatic deadline: S1 reclaims after lease
expiry with a new fence/authorization and original absolute send deadline rules. Unknown
send outcomes may duplicate delivery; stale results cannot restore bodies or quota.
No down migration or schema change is introduced. Empty-DB migration and populated restart
are exercised by B1; destructive rollback is not a recovery procedure.

`POST_RELAY_SENDER_ENABLED=false` still stops sending only (maintenance continues), and
`POST_RELAY_INBOX_ENABLED` still defaults to **false**. Neither flag is enabled as an outage
workaround. Watch fixed JWKS ready/unavailable/timeout states, refresh elapsed time, runtime
startup, stop/drain bound and deadline-exceeded logs together with queue/lease telemetry.

B1 acceptance is `cargo test -p server --features test-mode --test post_relay_runtime` with
`DATABASE_URL` set (missing DB fails, never skips). It uses real child processes, PostgreSQL,
HTTP-signature inbox requests and HTTP receivers, but seeds links directly; the real-user
registration/full relay E2E belongs to T1. The `test-mode`-only
`EMUMET_TEST_RELAY_ORIGINS` JSON peer list permits loopback HTTP solely in that test build;
the switch/constructor are compiled out of normal builds. Never deploy `test-mode` images.
