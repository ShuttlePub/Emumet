# S1 relay measurements

## Start baseline (2026-11-22)

Base: 7efe25c7b5e5237d4246e74b7c50903996040b14. Explicit release test:

    cargo test --release -p driver s1_baseline_real_http -- --ignored --nocapture

This is a **control harness**, not the final S1 sender or a production SLO: real Q1
claim/authorize/record transactions, a parsed RSA-2048 handle, SHA-256, and a local real
HTTP receiver. Admission/setup excluded from the measured interval; 128 deliveries/case.
Stored bodies are synthetic 65,536 and 1,400,000 bytes (maximum-body envelope scale).

Environment: AMD Ryzen 9 7900, 12 cores/24 threads; approximately 64 GiB host RAM;
Linux 7.3.5; release optimized Rust; 4 Tokio runtime threads, **not CPU-affinity-limited**.
PostgreSQL 17 on the same host (Unix socket, port 54329), disposable database per test;
16-connection baseline pool. Receiver on TCP loopback; **plain HTTP, no TLS**. Other host
workloads were not excluded. This differs from the target 4 vCPU/8 GiB deployment.

| bytes | parallel | deliveries/s | DB aggregate ms/delivery | digest + RSA ms/delivery | HTTP ms/delivery | process CPU ticks | RSS before → after KiB |
|---:|---:|---:|---:|---:|---:|---:|---:|
|65536|1|354.41|1.244|1.154|0.128|25|23980 → 42632|
|65536|16|330.35|37.485|1.194|0.130|24|42488 → 61136|
|1400000|1|192.59|2.132|1.691|0.966|49|93748 → 320500|
|1400000|16|275.56|46.774|1.769|1.304|51|320500 → 547292|

Global 16 / origin 4 slots are enforced by Q1. Four configured logical origins and 16
Profiles share one physical mock. DB time includes acquire/lock wait and origin-slot busy
retries, not an isolated lock-wait distribution. CPU is Linux /proc/self/stat user+system
ticks, excluding PostgreSQL. RSS includes allocator high-water retention and fixture/mock
overhead: these short runs **do not demonstrate stability**. All cases ended with zero live
bodies and reconciled reservations.

KDF was outside this control path (one freshly parsed handle); baseline KDF/cache rate,
queue wait, separate DB connection/lock wait, and unlink/deactivation/projector latency
are **not measured by this harness**. They must not be inferred as zero. The parallel DB
serialization cost increases markedly; shared quota/lifecycle transactions are the first
contention hypothesis, not a verified root cause.

## Acceptance targets (unchanged; final worker measurements pending)

4 vCPU/8 GiB, release: 64 KiB >=100/s; maximum-body envelope >=20/s; a healthy origin
maintains >=80% of its isolated rate beside a timeout origin; >=10 minutes with stable RSS,
connections and live rows. KDF must not scale with delivery count; HTTP must hold zero DB
connections/locks; unlink/deactivation/projector must not wait for slow HTTP.

The baseline is above throughput thresholds on a larger, non-TLS host but cannot be used
as acceptance evidence for the final worker or the target environment.


## Final sender, unrestricted host (2026-11-22)

Production application orchestration + Q1 PostgreSQL ports + production pinned HTTP adapter,
with test-mode loopback exception only. Four local streaming HTTP recipients, 16 Profiles,
128 deliveries per sample, four Tokio worker threads. Same Ryzen 9 7900 (12 physical / 24
logical cores), approximately 64 GiB host, release build, PostgreSQL 17 on the same host over
Unix socket. **Not CPU-affinity/memory limited; no TLS.** Sender and recipient share the
measurement process for this run; PostgreSQL is separate. Synthetic opaque immutable envelope
sizes are exactly 65,536 and 1,400,000 bytes (~1.34 MiB), not reserialized per attempt.

| Envelope | Concurrent | Delivery/s | process CPU ticks (100 Hz) | RSS before → after, KiB | KDF calls / decrypt ms |
|---|---:|---:|---:|---:|---:|
| 65,536 | 1 | 267.00 | 40 | 15,872 → 30,372 | 1 / 67.18 |
| 65,536 | 16 (4/origin) | 538.22 | 46 | 30,364 → 36,972 | 1 / 67.26 |
| 1,400,000 | 1 | 173.07 | 63 | 36,880 → 69,768 | 1 / 59.98 |
| 1,400,000 | 16 (4/origin) | 311.33 | 77 | 62,368 → 83,700 | 1 / 61.96 |

| Envelope / concurrency | DB phase ms/job | local DB-slot wait ms/job | RSA+digest ms/job | HTTP ms/job | auth-start→HTTP ms/job |
|---|---:|---:|---:|---:|---:|
| 65,536 / 1 | 2.017 | 0.0004 | 1.114 | 0.599 | 0.526 |
| 65,536 / 16 | 5.290 | 10.016 | 1.646 | 4.987 | 5.379 |
| 1,400,000 / 1 | 3.181 | 0.0004 | 1.703 | 0.878 | 1.063 |
| 1,400,000 / 16 | 10.874 | 24.653 | 2.215 | 5.257 | 12.780 |

Rates time delivery after preload, including claim/prepare/unlock/authorize/commit/HTTP/result,
but exclude admission. CPU tick/RSS snapshots include admission and the recipient; do not
interpret CPU ticks/rate as identical measurement windows. DB-phase includes pool acquisition,
queries/gates/locks and unlock bookkeeping; local semaphore wait is separate. It is **not an
isolated PostgreSQL connection-wait or lock-wait distribution**. No such distribution has yet
been collected. Signing is performed before authorization. KDF counts are actual decrypts,
not cache lookups; parsed signer preparation is separately asserted once by correctness tests.

Continuous 65,536-byte batches of 64, bounded 16, for **600.09s**: **204,288 successes,
340.43/s including continuing admission**. RSS 54,964 → 56,320 KiB; one KDF for all deliveries;
body rows zero at each completed-batch sample; sampled lock waiters zero; test DB connections
7 → 12 (observed max 13, pool cap 16). Connection creation gradually filled the bounded pool,
not an indefinitely growing connection series. CPU 390 → 65,686 ticks (about 1.09 cores mean,
including recipient/admission, excluding DB). RSS at 60/300/600 seconds: 55,456/55,688/56,320 KiB.
This demonstrates bounded steady-state in this environment, not all production volumes/loads.

### Measurement corrections and implementation finding

The first two long runs were deliberately stopped: wiremock 0.6.5 MountedMock retains
matched request bodies even with disable_request_recording. Its matched_requests Vec grew
with every body; this is a **measurement-recipient leak**, not evidence of a sender leak.
Those RSS series are invalid for the sender stability goal. The final run uses a streaming
TCP HTTP recipient which discards bodies in 8 KiB chunks and stores no per-request history.

Separately, creating a fresh native-TLS reqwest client every attempt cost approximately
16ms/request even without TLS traffic (initial final-sender 64KiB serial 58.52/s, parallel
202.24/s). The sender now bounds client caching at 64 entries/60s idle, keyed by the **freshly
validated origin/address set**, with DNS rechecked before reuse. This increased serial rate
to 267.00/s without weakening private-IP/rebinding checks. The unrelated wiremock retention
must not be attributed to this cache optimization.

A rolling-refill isolation probe also exposed overclaiming before earlier tasks finished
signing/authorization: permit occupancy alone omitted live preparation leases. Official
scheduled claims now serialize their short snapshot/update and count unexpired unauthorized
leases as provisional occupancy (global16/origin4), backed by a partial lease index. HTTP
never owns that scheduler lock. Authoritative Q1 permit slots/fencing are unchanged. Expired
preparation leases recover after the original 30s; stale workers still cannot authorize/send.
The regression test checks global/origin caps before any authorization and across refills.

## Correctness evidence map

All tests use a real PostgreSQL database when DATABASE_URL is supplied. The performance tests
are ignored by default and must be invoked explicitly with --release --ignored --nocapture.
Do not mistake an ignored performance test or missing DATABASE_URL for validation.

- application instance_identity::tests::post_relay_delivery:
  - s1_p0_p1_p2_p4_three_revokers_auto_and_manual_no_body_resurrection: 24 permutations
    (unlink / Account command-event / actual Account projector, automatic/manual). Authorization
    commit wins P1/P2; unlink wins P0; P4 cannot resurrect deleted body/quota/history.
  - s1_unlink_gate_and_precommit_checkpoints_three_revokers_auto_manual: 12 permutations,
    DB-local BEFORE/AFTER state-update pause after common gate acquisition, sender blocked
    until unlink commit, zero HTTP. Hooks/triggers exist only in each disposable test DB.
  - s1_p3_silent_http_has_zero_db_holds_and_all_revokers_finish: six cases, real HTTP peer
    delays response 30s; unlink finishes while HTTP remains pending. Tests assert all pool
    connections idle, no idle transaction/advisory lock, and no body after maintenance.
  - s1_p1_timeout_rollback_does_not_send_and_p2_expired_does_not_send: failed/unknown-to-sender
    authorization completion never sends; post-commit 11s scheduling pause never starts HTTP.
  - s1_outgoing_headers_match_canonical_cavage_vectors; own-public-key verification and late
    result test; 401 retry test; in-time P4 observation persisted after the 10s deadline.
- driver post_relay_delivery::tests: raw bytes/status/Retry-After/redirect, absolute DNS+HTTP
  timeout, already-expired no-connect, internal/mixed addresses/trailing-dot, pinned DNS,
  cached public target cannot bypass a rebound private answer.
- driver post_relay::tests::scheduler: Profile rotation/origin fairness/saturated origin,
  original immutable near-deadline receipt priority, provisional occupancy across refills.
- Q1 regression suite: manual one-shot/crash/competing operation, attempt/lease fencing,
  72h/30d boundaries, delayed in-time observation across cutoff, stale cancellation/purge,
  no success/cancellation terminal rows, audit/quota/physical-capacity invariants.
- server tests/post_relay_operator.rs: shipped binary preview, wrong-confirm rollback,
  confirmed discard/audit, empty redelivery, schema mismatch without migration, unusable
  WORKER_ID and unrelated service config. Optional S1_OPERATOR_IMAGE runs the same cases via
  the release image with network disabled and only DB socket/password mounted.

The initial workspace run hit 78 setup failures because the shared postgres root DB retained
an earlier Q1 checksum. Existing migrations/shared DB were not edited to suppress it. The
complete rerun uses a newly created dedicated root DB. The pre-Q1 upgrade fixture was adjusted
to exclude Q1 **and its dependent later migrations**, retaining the populated-main upgrade test.

## Four-core / 8 GiB address-space measurement (2026-11-22)

Same Ryzen 9 7900 host, release build, code identical to the final head except for test-only
changes. **Sender process (production worker + Q1 ports + pinned HTTP adapter + measurement
harness) pinned with `taskset -c 0-3` and `ulimit -v 8388608` (8 GiB virtual address space);
PostgreSQL 17 pinned to cores 4-7 (separate process, Unix socket); the HTTP recipient is an
external child process pinned to cores 8-9** (so recipient CPU does not compete with the
sender). Four Tokio worker threads, 16 Profiles, four logical origins, 128 deliveries per
sample, plain HTTP, no TLS. Other lanes' workloads were not excluded from the host.

| Envelope | Concurrent | Delivery/s | CPU ticks | RSS before → after, KiB | KDF calls |
|---|---:|---:|---:|---:|---:|
| 65,536 | 1 | 224.11 | 42 | 15,744 → 30,316 | 1 / 59.4 ms |
| 65,536 | 16 (4/origin) | 416.80 | 38 | 30,304 → 37,540 | 1 / 59.6 ms |
| 1,400,000 | 1 | 155.11 | 59 | 37,444 → 62,852 | 1 / 58.8 ms |
| 1,400,000 | 16 (4/origin) | 264.63 | 65 | 55,452 → 87,532 | 1 / 64.2 ms |

An earlier identical-configuration run gave 219.84 / 371.50 / 153.93 / 267.86 per second, i.e. the
run-to-run spread is roughly ±10 %. Per-job phases (ms, mean over 128 jobs; DB phase includes
pool acquire, queries, gates and unlock bookkeeping; the semaphore wait is separate):

| Envelope / concurrency | DB phase | local DB-slot wait | RSA+digest | HTTP | auth-start→HTTP |
|---|---:|---:|---:|---:|---:|
| 65,536 / 1 | 2.66 | 0.0004 | 1.20 | 0.59 | 0.72 |
| 65,536 / 16 | 7.50 | 14.65 | 2.25 | 4.71 | 7.81 |
| 1,400,000 / 1 | 3.68 | 0.0004 | 1.75 | 0.99 | 1.24 |
| 1,400,000 / 16 | 13.63 | 30.41 | 3.29 | 3.79 | 15.52 |

### Ten-minute steady run (same four-core / 8 GiB-AS configuration)

65,536-byte envelopes, batches of 64, bounded 16, **600.12 s, 172,032 successes, 286.66/s
including continuing admission** (admission shares the sender cores). Sampled once a minute:

| elapsed s | successes | RSS KiB | CPU ticks | live bodies | DB connections | lock waiters | KDF calls |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 0.3 | 64 | 59,540 | 346 | 0 | 8 | 0 | 1 |
| 60 | 18,176 | 60,608 | 4,996 | 0 | 10 | 0 | 1 |
| 120 | 36,032 | 62,708 | 9,619 | 0 | 10 | 0 | 1 |
| 300 | 86,144 | 62,732 | 22,815 | 0 | 10 | 0 | 1 |
| 600 | 172,032 | 62,760 | 45,109 | 0 | 10 | 0 | 1 |

RSS is flat after the first two minutes (62,708 → 62,760 KiB over the last 480 s), connections
stay at 10 (pool cap 16), live body rows and lock waiters are 0 at every sample, one KDF for all
172,032 deliveries (so KDF does not scale with deliveries). All relay tables were sampled each
minute (`S1_TABLE_COUNTS`): jobs, bodies, attempts, dead letters, audits, operations and
unlinked rows stayed 0 and the fixed rows (slots 16, schedule 16, profile quota 16, quota/
capacity/counters 1) never changed, so there is no row growth after terminal results.
CPU averaged about 0.75 core (44,763 ticks / 600 s, excluding PostgreSQL and the recipient).

### Healthy origin beside a timing-out origin (four-core / 8 GiB-AS)

Three healthy origins and one origin whose receiver never answers (timeouts), 12 s windows,
median of three rounds, alone vs mixed, 64 KiB envelopes:

| round | alone/s | mixed/s (slow-origin timeouts) |
|---:|---:|---:|
| 0 | 255.92 | 240.61 (4) |
| 1 | 223.40 | 144.16 (4) |
| 2 | 158.65 | 222.56 (4) |

Median alone 223.40/s, mixed 222.56/s: **ratio 0.9963 (target ≥ 0.80)**. The rounds themselves
vary widely (144–256/s) in both arms because of co-tenant host load, so the ratio is a median
over a noisy sample, not a tight estimate. Only healthy-origin deliveries are counted; the
harness now routes healthy traffic to the three healthy origins only.

### Unlink / deactivation latency while the sender is in each stop point

From `s1_p0_p1_p2_p4_…` and `s1_p3_silent_…` (real PostgreSQL, four-core / 8 GiB-AS):
revocation (3 revokers × automatic/manual × P0/P2/P4) completes in **1.3–10.9 ms**; point P1
(which deliberately holds the permission gate) 41–55 ms of which 20 ms is the test's wait to
prove the revoker is blocked. With the HTTP peer silent for 30 s (P3) all three revokers
finish in **1.3–6.7 ms** while the HTTP request is still pending, with zero idle transactions,
zero advisory locks and an all-idle pool. The 3 s test bound was never approached.

## Target comparison

| Plan §7.3 target | Result | Verdict |
|---|---|---|
| 64 KiB ≥ 100/s (4 vCPU, 8 GiB, release) | 220–224/s serial, 372–417/s bounded-16 (4 cores, 8 GiB-AS); 267–538/s on the unrestricted host | met (plain HTTP) |
| ~1.34 MiB envelope ≥ 20/s | 154–155/s serial, 264–268/s bounded-16; 173/311 unrestricted | met (plain HTTP) |
| healthy origin ≥ 80 % beside a timing-out origin | 0.9963 (median of 3), earlier run 0.9966 | met |
| ≥ 10 min with stable RSS / connections / live rows | 600 s, RSS flat, 10 connections, 0 live bodies/rows, 0 lock waiters | met |
| KDF not proportional to deliveries | 1 KDF per 128 and per 172,032 deliveries | met |
| unlink/deactivation/projector do not wait for slow HTTP | 1.3–6.7 ms with a 30 s silent peer | met |

**No target is missed in the measured environment, but the environment differs from the target
in the following ways, none of which this document claims to have closed:**

- **No TLS.** Recipients are plain loopback HTTP; TLS handshake/crypto/session-reuse and real
  network RTT are not measured and can reduce throughput. The per-origin 4 / global 16 caps
  were not raised to disguise that.
- **The 8 GiB limit is a virtual address-space `ulimit -v`, not a cgroup memory controller**;
  RSS (≈ 60–90 MiB) is far below it but cgroup page-cache/accounting behaviour is not covered.
- **CPU pinning is physical-core selection on a 12-core host (cores 0–3 = four distinct physical cores, one hardware thread each, for the
  sender), not a 4-vCPU VM**; PostgreSQL and the recipient run on separate cores, so a real
  4-vCPU deployment that co-locates PostgreSQL would see lower rates. Run-to-run spread is
  ±10 % and co-tenant load was not excluded.
- **Pure DB connection-wait / lock-wait distributions were not collected**; only the aggregate
  DB-phase and local semaphore times above, plus sampled lock-waiter counts (0), exist.
- Production volumes and mixed workloads beyond four logical origins / 16 Profiles are untested.
