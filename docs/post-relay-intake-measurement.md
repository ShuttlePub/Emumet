# I1 intake measurement (2026-11-22)

Code: feat/i1-profile-inbox-relay, implementation 6506f5b (Q1 base
7efe25c7b5e5237d4246e74b7c50903996040b14). Independent from S1 #123/#125.

## Environment and method

- Linux 7.3.5-cachyos, AMD Ryzen 9 7900 (24 logical CPUs), host RAM 62 GiB.
  Receiver and PostgreSQL both pinned to the SAME logical CPUs 0-3 (four total), not four each.
  No 8 GiB cgroup memory limit: this is explicitly NOT the exact 4-vCPU/8-GiB production environment.
- Rust release profile, Tokio four runtime worker threads. PostgreSQL 17.9, same machine,
  dedicated throw-away cluster, Unix socket :54331, local NVMe filesystem. fsync,
  synchronous_commit and full_page_writes all **on**. Other host work was running concurrently.
- Production Axum request handler in-process: real RSA-2048 verification, bounded JSON/evidence,
  same Q1 transaction gates/quota/rate/hour buckets, dedicated max-4 intake connection pool.
  No TCP/TLS, remote actor fetch, proxy, or S1 sender cost. A registered test peer key represents a
  cached remote key. Sender signing, schema migrations and Profile/key provisioning excluded.
- Raw body 49,289 bytes (48 KiB content); linked envelope about 66 KiB including evidence/Base64.
  1,024 requests/condition, bursts of 16 at 160 ms intervals, bounded <=16 active, offered 100/s.
  Initial burst and completion make measured elapsed throughput slightly above offered rate;
  these are NOT saturation-capacity numbers. Each condition lasts about ten seconds, not ten minutes.
- CPU: process user+system ticks (CLK_TCK=100), includes probe scheduling/sampling, excludes the DB
  process. RSS: sampled receiver process. DB Lock wait sampling ~10 ms; not complete lock duration.
  receiver_pool_acquire is an actual timed acquire; transaction_begin includes pool wait plus BEGIN
  round trip (upper bound on pure connection wait). No unbounded actor/receipt labels are used.

## Results

All 4,096 requests returned **202**, no 429/503 or storage errors. Reservation reconciliation was
true after every condition. No sender was running. Unlinked cases retained only one/eight hourly
buckets; linked cases retained 1,024 jobs each with capacity reserved.

| Link | Profiles | req/s | 202 p50/p95/p99/max (ms) | CPU seconds | RSS peak (MiB) | Lock samples / all; max waiters | pool in use peak |
|---|---:|---:|---|---:|---:|---|---:|
| unlinked | 1 | 101.29 | 18.10 / 32.88 / 43.72 / 52.13 | 1.72 | 48.93 | 152/904; 3 | 4 |
| unlinked | 8 | 101.26 | 17.07 / 51.05 / 140.22 / 153.78 | 1.88 | 50.24 | 140/904; 3 | 4 |
| linked | 1 | 101.21 | 28.44 / 47.21 / 55.99 / 66.55 | 1.95 | 50.52 | 224/906; 3 | 4 |
| linked | 8 | 101.23 | 22.04 / 37.61 / 45.42 / 76.55 | 1.94 | 51.20 | 171/907; 3 | 4 |

Phase p95 / p99 in milliseconds (transaction time includes receiver lookup, locks, reservation, commit):

| Link/profiles | signature | reading/evidence | admission | receiver acquire | tx begin (wait+BEGIN) |
|---|---|---|---|---|---|
| unlinked/1 | 0.295 / 1.539 | 0.043 / 0.685 | 32.535 / 42.951 | 3.059 / 7.945 | 21.768 / 29.651 |
| unlinked/8 | 0.295 / 1.131 | 0.045 / 0.169 | 50.851 / 139.852 | 2.798 / 5.437 | 24.181 / 74.664 |
| linked/1 | 0.294 / 1.026 | 0.038 / 0.067 | 46.696 / 54.559 | 2.278 / 4.430 | 35.614 / 42.091 |
| linked/8 | 0.296 / 0.869 | 0.037 / 0.105 | 37.137 / 44.284 | 2.191 / 5.024 | 27.239 / 32.664 |

## Interpretation and limits

This bounded probe sustained the plan's **100 requests/s offered intake** in all four conditions,
including shared quota and same-hour-bucket contention, with physical commit durability enabled.
It does not establish maximum throughput, maximum-body intake, a 10-minute steady-state sender
capacity, RSS stability over hours, DB WAL/vacuum capacity, cold remote-key fetching/TLS latency,
or an unconditional production SLO. No inference about S1 delivery target attainment is made.
S1 merge plus the lead's performance/activation decision remain prerequisites. Default intake
flag remains OFF.

An earlier exploratory run used the provided shared :54329 PostgreSQL (fsync=off); it is NOT the
result above. The final run uses the private fsync=on cluster and final dedicated-pool wiring.
The first full workspace test attempt against the shared default DB encountered pre-existing Q1
migration checksum drift. No checksum was edited: a new I1-owned workspace database was used.

Reproduce using the release probe command in post-relay-operations.md with DATABASE_URL pointing
to an isolated durable PostgreSQL and the receiver/DB on the same four logical CPUs. The complete
fixed-field measurements below preserve the phase extrema and sampling limitations:

```json
[
  {
    "concurrency_bound": 16,
    "cpu_ticks": 172,
    "db": {
      "lock_samples": 152,
      "max_lock_waiters": 3,
      "max_pool_in_use": 4,
      "rss_peak_kib": 50104,
      "samples": 904
    },
    "hour_buckets": 1,
    "jobs": 0,
    "latency_us": {
      "max": 52129,
      "p50": 18103,
      "p95": 32883,
      "p99": 43721
    },
    "linked": false,
    "profiles": 1,
    "raw_bytes": 49289,
    "requests": 1024,
    "rps": 101.28580095831131,
    "seconds": 10.110005453,
    "stages_us": {
      "accepted_us": {
        "max": 52109,
        "p50": 17984,
        "p95": 32835,
        "p99": 43645
      },
      "admission_us": {
        "max": 51891,
        "p50": 17788,
        "p95": 32535,
        "p99": 42951
      },
      "inbox_receiver_pool_acquire_us": {
        "max": 40496,
        "p50": 1335,
        "p95": 3059,
        "p99": 7945
      },
      "read_us": {
        "max": 2096,
        "p50": 23,
        "p95": 43,
        "p99": 685
      },
      "signature_us": {
        "max": 4194,
        "p50": 162,
        "p95": 295,
        "p99": 1539
      },
      "transaction_begin_us": {
        "max": 38905,
        "p50": 8948,
        "p95": 21768,
        "p99": 29651
      }
    },
    "status": {
      "202": 1024
    },
    "target_rps": 100
  },
  {
    "concurrency_bound": 16,
    "cpu_ticks": 188,
    "db": {
      "lock_samples": 140,
      "max_lock_waiters": 3,
      "max_pool_in_use": 4,
      "rss_peak_kib": 51448,
      "samples": 904
    },
    "hour_buckets": 8,
    "jobs": 0,
    "latency_us": {
      "max": 153775,
      "p50": 17070,
      "p95": 51052,
      "p99": 140222
    },
    "linked": false,
    "profiles": 8,
    "raw_bytes": 49289,
    "requests": 1024,
    "rps": 101.2622125946611,
    "seconds": 10.112360512,
    "stages_us": {
      "accepted_us": {
        "max": 152313,
        "p50": 17011,
        "p95": 51033,
        "p99": 140178
      },
      "admission_us": {
        "max": 152037,
        "p50": 16832,
        "p95": 50851,
        "p99": 139852
      },
      "inbox_receiver_pool_acquire_us": {
        "max": 22248,
        "p50": 1336,
        "p95": 2798,
        "p99": 5437
      },
      "read_us": {
        "max": 3025,
        "p50": 25,
        "p95": 45,
        "p99": 169
      },
      "signature_us": {
        "max": 3027,
        "p50": 170,
        "p95": 295,
        "p99": 1131
      },
      "transaction_begin_us": {
        "max": 145469,
        "p50": 8983,
        "p95": 24181,
        "p99": 74664
      }
    },
    "status": {
      "202": 1024
    },
    "target_rps": 100
  },
  {
    "concurrency_bound": 16,
    "cpu_ticks": 195,
    "db": {
      "lock_samples": 224,
      "max_lock_waiters": 3,
      "max_pool_in_use": 4,
      "rss_peak_kib": 51728,
      "samples": 906
    },
    "hour_buckets": 0,
    "jobs": 1024,
    "latency_us": {
      "max": 66550,
      "p50": 28437,
      "p95": 47209,
      "p99": 55991
    },
    "linked": true,
    "profiles": 1,
    "raw_bytes": 49289,
    "requests": 1024,
    "rps": 101.21382139148163,
    "seconds": 10.117195319,
    "stages_us": {
      "accepted_us": {
        "max": 66490,
        "p50": 28352,
        "p95": 47137,
        "p99": 55690
      },
      "admission_us": {
        "max": 65343,
        "p50": 28051,
        "p95": 46696,
        "p99": 54559
      },
      "inbox_receiver_pool_acquire_us": {
        "max": 30633,
        "p50": 1302,
        "p95": 2278,
        "p99": 4430
      },
      "read_us": {
        "max": 1648,
        "p50": 23,
        "p95": 38,
        "p99": 67
      },
      "signature_us": {
        "max": 4281,
        "p50": 162,
        "p95": 294,
        "p99": 1026
      },
      "transaction_begin_us": {
        "max": 53698,
        "p50": 17104,
        "p95": 35614,
        "p99": 42091
      }
    },
    "status": {
      "202": 1024
    },
    "target_rps": 100
  },
  {
    "concurrency_bound": 16,
    "cpu_ticks": 194,
    "db": {
      "lock_samples": 171,
      "max_lock_waiters": 3,
      "max_pool_in_use": 4,
      "rss_peak_kib": 52424,
      "samples": 907
    },
    "hour_buckets": 0,
    "jobs": 1024,
    "latency_us": {
      "max": 76552,
      "p50": 22044,
      "p95": 37612,
      "p99": 45417
    },
    "linked": true,
    "profiles": 8,
    "raw_bytes": 49289,
    "requests": 1024,
    "rps": 101.22957800091189,
    "seconds": 10.115620555,
    "stages_us": {
      "accepted_us": {
        "max": 76530,
        "p50": 22001,
        "p95": 37346,
        "p99": 45394
      },
      "admission_us": {
        "max": 76338,
        "p50": 21825,
        "p95": 37137,
        "p99": 44284
      },
      "inbox_receiver_pool_acquire_us": {
        "max": 34204,
        "p50": 1296,
        "p95": 2191,
        "p99": 5024
      },
      "read_us": {
        "max": 1670,
        "p50": 21,
        "p95": 37,
        "p99": 105
      },
      "signature_us": {
        "max": 4000,
        "p50": 162,
        "p95": 296,
        "p99": 869
      },
      "transaction_begin_us": {
        "max": 54353,
        "p50": 13095,
        "p95": 27239,
        "p99": 32664
      }
    },
    "status": {
      "202": 1024
    },
    "target_rps": 100
  }
]
```
