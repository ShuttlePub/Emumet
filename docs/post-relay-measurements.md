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
