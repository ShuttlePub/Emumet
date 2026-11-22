# Profile inbox relay intake (I1)

Contract: host-owned v1 revision 3 in docs/contracts (pinned snapshot, not edited here).
Base: 7efe25c7b5e5237d4246e74b7c50903996040b14 (Q1, PR #122).

## Deployment gate — default OFF

POST_RELAY_INBOX_ENABLED defaults to false (unset = false); only the literal true enables durable
relay admission. Other values fail startup. Enabling requires a normalized HTTPS PUBLIC_BASE_URL
origin (lowercase ASCII/Punycode DNS, no path/trailing slash/userinfo/query/fragment,
no explicit default port; at most 255 bytes). Profile identity and signing are not changed.

**Why it is off by default:** the ShuttlePub receiver for `POST /internal/v1/emumet/activities`
(receive, verify, de-duplicate, store) does not exist yet. Until it does, keep the flag off:
turning it on would only make every delivery fail and pile up in the 30-day quarantine. It is
**not** an operational safety valve. (An earlier revision of this page called it one; that was
wrong. See host decision D68, `intents/shuttlepub/decisions/2026-11-21-emumet-p1-open-items.md`,
which also states the conditions for turning it on: the ShuttlePub receiver is implemented and
deployed; the preparation below is complete; and a federated E2E between both sides passes.)
The T1 E2E (`server/tests/e2e_post_relay.rs`) sets the flag to true against a mock P1 receiver;
that does not change the default.

With the flag unset no relay jobs or unlinked relay aggregates are admitted; existing Follow
processing and early-Undo defense continue. Turning the flag off affects NEW admission only, not
accepted jobs, retries or authorized HTTP sends. It is not an operator emergency stop for S1;
stop all sender/operator processes or block egress for that.

Before enabling: apply additive migrations, verify S1 delivery/maintenance is deployed, supply
fresh DB-volume capacity samples (Q1 capacity-report port/CLI; never container free disk),
configure POST_RELAY_ALLOWED_ORIGINS, provision instance keys and register the owner-approved
home links. No origin is learned from an incoming Host/Forwarded header. Q1 unknown/stale
physical capacity fails closed. See post-relay-storage.md for capacity/retention/recovery ports.

## Acceptance boundary

1. Bound raw body to 1 MiB and request target to 2048 characters. Check ActivityPub JSON or
   ld+json MIME (UTF-8 charset; ld+json AS profile or omitted profile).
2. Verify the signature against raw bytes. Use the key ID/owner/scheme/time returned by that
   actual cryptographic verification, NOT a second key fetch. Compare owner and key-document
   actor against the minimally read actor. No JSON-LD context or object URL is fetched.
3. Reject duplicate keys at every JSON level; accepted depth <=32, nodes <=100000, type and
   actor arrays <=16. URL_MAX=2048, TYPE_STRING_MAX=512. Invalid/overlong Activity ID becomes
   null; invalid actor is 400 invalid_actor (missing/ambiguous actor has its own fixed code).
   Unknown types have no allowlist. to/cc strings and object arrays do not gate forwarding.
4. Exclude Follow/Block themselves. Exclude Undo/Accept/Reject only if EVERY nonempty reference
   is a certain embedded or stored Follow/Block. Missing/empty/mixed/unknown references forward
   the entire body. Relationship processing runs only on a uniquely interpretable operation.
5. Use an independent process-wide intake pool (maximum 4, lazy, same database/options,
   no extra migration run), preserving shared API/projector pool capacity. In ONE Q1 transaction/connection: Account/Profile/relationship/link gates and owner recheck,
   necessary relation receipt/early-Undo work, quota/rate reservation, immutable job bytes OR
   body-free unlinked hourly aggregate. Error/overload rolls EVERYTHING back. 202 follows commit.

A receiver-scoped early-Undo marker (and a synthetic ended Follow generation) is not proof of
an actual received Follow. Migration 20261122000500 adds observed_relation to the existing
relation-receipt table; only actual authenticated Follow/Block sets it, including a late Follow
that hits an UndoFollow duplicate. The marker's original type remains UndoFollow so it is not
purged or revived. Existing outbound local Follow generations are also certain references.
A Duplicate result suppresses relationship mutation, NEVER relay admission. Lost committed
202 responses may create another receipt/envelope on retry; at-least-once, not exactly-once.

Raw bytes are Base64 encoded without Activity reserialization. The final envelope is serialized
once for Q1, fixing origin/link/generation/receipt/DB received_at. S1 reuses those exact bytes.
Unlinked post reception stores no body, headers, receipt, peer actor/Activity ID or origin;
only Profile + no_home + UTC hour bucket counts/times. Existing relation receipts/early-Undo
cancellation state remain necessary for Follow safety, and are not unlinked relay audit data.

## Responses, limits and observations

- 202: relation operation committed, or relay job/hourly audit committed (when enabled).
- 400: malformed JSON/MIME/actor/type, duplicate keys, depth/node bounds, ambiguous relation
  operations (two recognized relation terms). 413: body >1 MiB.
- 401: signature failure, unsupported signature scheme, signer/actor mismatch. Existing
  Cavage verification is preserved; this unit does not add RFC 9421 verification.
- 404: missing/non-actor Profile or non-active owner. 408: retryable Follow actor resolution.
- 422: known Follow relationship rule violation, no relationship changes.
- 429 + Retry-After: 1: Profile rate/quota exhaustion.
- 503 + Retry-After: 1: global/audit/physical capacity or storage failure/unknown commit.
  A retry is required; receipt/guard/reservations were not acknowledged as durable.

Q1 shared PostgreSQL buckets remain 1000 requests/s global per admission partition,
200 requests/s/Profile, bursts 1000/200. No process-local substitute is used. Quota and hourly
bucket contention are included in the release probe. See post-relay-storage.md for the common
job/quarantine bounds, protected audit partitions and physical-capacity freshness/hysteresis.

The 16 KiB evidence budget is NOT the HTTP parser's incoming header limit. Axum/Hyper and the
front proxy enforce their own field/count/buffer limits before this route; deployment must
configure/observe those ingress limits separately. This route does not raise them to fit
oversized evidence. Within accepted requests, evidence omits whole candidates under rev 3's
64-field/16 KiB/name/count limits; never truncates values. Secret-bearing covered headers are
omitted with evidence_complete=false. RFC 9421 escaped component names are parsed for this
evidence algorithm, independently of supported cryptographic verification schemes.

No body, Cookie/Authorization, key material or peer response body is logged. Read/DB errors use
fixed responses, not serde/SQL diagnostic bodies. Debug events contain only fixed numeric
signature_us/read_us/admission_us/accepted_us, inbox_receiver_pool_acquire_us and generic
transaction_begin_us (pool acquisition + BEGIN round trip, a conservative wait upper bound).
Do not turn actor/receipt/Activity ID into metrics labels. Monitor 429/503, Q1 quota/capacity age,
DB gate timeouts and purge delay; never auto-discard admitted content to make room.

## Validation and measurement

The production reader and evidence builder run against all C0 inbox/header fixtures in
post_relay_contract. profile_relay_tests exercise schema/raw-byte identity, atomic rollback
and retries (SQL fault after receipt/guard), old receive-only duplicates, distinct recipients,
D63 unknown/mixed/empty arrays, early-Undo non-evidence, overload, concurrent hourly buckets,
the final Profile quota slot and populated additive migration backfill. Existing inbox,
Follow, domain-control and WebFinger tests remain required with DATABASE_URL set.

Run the explicit release probe (never count a skipped DB test as passing):

    cargo test --release -p server --bin server measure_i1_intake_shared_quota_and_hour_bucket -- --ignored --nocapture

It sends 1024 pre-signed requests per condition, 16-request bursts every 160 ms (offered
100 requests/s, <=16 active), 48 KiB content (~64 KiB envelope), actual RSA verification,
production Axum handler and real PostgreSQL, linked/unlinked x one/eight Profiles. Peer key
lookup uses a test fixture (cached remote key); no remote HTTP/TLS/key retrieval is measured.
Setup and sender signing are excluded. Status counts, p50/p95/p99/max phase/total times,
CPU ticks, sampled RSS, pool use, sampled PostgreSQL Lock waits and surviving rows are printed.
The driver begin metric includes BEGIN itself; it is NOT claimed as pure pool wait. DB wait
sampling is not a full lock-duration trace. The probe is bounded intake evidence, not a 10-minute
sender/backlog test, not a 4-vCPU production SLO, and does not authorize S1/I1 activation.

Measured environment/results are recorded in post-relay-intake-measurement.md.

## Inbox URL bounds versus the legacy relation graph

The forwarding contract accepts actor, usable Activity ID, verification key ID and the **actual
verified owner** through URL_MAX=2048. The owner is validated itself before admission: the slash-tolerant
owner/body-actor comparison is not URL validation. Envelope evidence always retains that actual owner;
it is never replaced with the body actor. Legacy relation authorization/cancellation also uses the
verified owner, including when the body actor differs by a trailing slash.

The pre-existing social-graph types and database constraints remain 1024 bytes. Do not apply those
representation limits to forwarding. Values beyond that boundary skip legacy relation mutation/receipt
work instead of causing a 400 or creating a relation under another identity. A valid 1025–2048 Activity
ID remains unchanged in the envelope, and an ID over URL_MAX becomes null. No existing graph relation
can belong to a >1024-byte owner, or have a >1024-byte Follow ID. An outer Undo ID that cannot fit the
legacy receipt must not discard its short Follow reference: omit only that outer legacy receipt and
keep the receiver-scoped early-Undo guard in the same Q1 transaction. Raw bytes/the original usable
outer ID still reach the durable relay job.

Type classification and relation authorization are separate. Stored Follow/Block evidence is looked up
by receiving Profile + Activity ID, across all signers; a different signer never gains authority to
change that relation. Guard markers alone are not evidence, and evidence from another Profile does
not exclude forwarding. The D63 routing result remains independent of legacy representation limits.
No pre-existing graph database constraint or index was changed for this separation.
