# P1 home links (L1)

L1 implements the owner-facing ledger only. It does **not** enable inbox relay acceptance,
create relay jobs/quarantine, start a relay worker, or change Profile delivery. The host's
emumet-post-relay v1 contract remains canonical; this document describes implementation choices.

## API and configuration

JWT and the existing optional `X-Organization-Id` context protect:

- `POST /api/v1/profiles/{profile_nanoid}/links`: `{ "origin": "https://home.example", "trust_mode": 3 }`.
  201 new, 200 same normalized active home, 409 another active home/ownership conflict.
  Only mode 3 is supported (other modes: 422); malformed/non-allowed origins: 400.
- `GET /api/v1/profiles/{profile_nanoid}/links`: `{ "items": [...] }`, current owner only,
  including unlinked ledger entries.
- `DELETE /api/v1/profiles/{profile_nanoid}/links/{link_id}`: 204, also for an already-unlinked
  ID. An old ID never targets a newly registered home. Failed DB work never returns 204.

`POST_RELAY_ALLOWED_ORIGINS` is a JSON string array, e.g.
`["https://home.example","https://other.example:8443"]`. Unset/[] denies registration;
invalid configuration fails startup. Origins are HTTPS, IDNA/lowercase normalized, default port
removed, and matched exactly (no wildcard/subdomain expansion). Only absent/root paths are
accepted; userinfo/query/fragment, internal IP literals and non-public DNS answers are rejected.
DNS resolution occurs **before** opening the mutation transaction. No registration HTTP probe is
made. `HomeOriginAllowlist::prepare_operation` constructs a no-proxy, redirect-free client pinned
to all validated addresses; S1 must call it again for the **actual** HTTP operation, not reuse a
registration-time DNS approval. Removing an origin/eligibility failure does not unlink the ledger:
S1 treats it as policy hold, rechecking owner/link/deadline on recovery without extending deadlines.

A Profile need not have its one-time username yet to register a home. This was confirmed by the
parent in question-2e1578885ec63c95572a82436f587b2e: registration requires an active owner;
I1/S1 still refuse non-actor inbox/send eligibility under the existing federation rules.
Current owners may GET/DELETE while suspended or banned; only registration/send/manual-redelivery
require active eligibility (parent decision question-146fc797f523469ca5c83ce8daca1f33). The normal JWT
and organization-context middleware is unchanged; no authentication exception is introduced.

## Authority and replay boundary

The authoritative Account lifecycle source is **the complete `account_events` stream**, reconstructed
under its Account gate, **not** the eventually consistent `accounts` projection. This applies to
registration, ownership transfer, unlink and the reusable permission-check port. Suspension expiry
is evaluated against current time. BAN/suspension hold the link (ID/generation unchanged).
Incomplete streams (including seq commit inversion) fail closed for authorization; the projector
warns and preserves its existing window retry rather than falling back to a projection.
If a historical Created event refers to an AuthAccount that no longer exists, projection continues
without recreating its auth linkage or Keto owner relation; unrelated DB/lifecycle errors still roll back.

Current Profile ownership is rechecked on its locked row; normal transfer writes its Profile event,
Profile row and ledger ownership in one transaction. The pre-gate owner is only a candidate.
A changed candidate retries from discovery in a **new transaction** (at most three attempts);
persistent concurrency returns 409. Never acquire a new, possibly lower Account gate while
retaining the old one.
An organization requires the existing organization context; a personal auth linkage alone does
not grant organization-context access.

`profile_links` has no FK to the disposable Account/Profile projections. The immutable Profile ID,
actor snapshot and link ID remain after Profile deletion. A partial unique index enforces one
active mode-3 home. Registration creates generation 1; unlink increments once, repeated unlink
never increments. Re-registration/origin change uses a new ID. Transfer preserves ID/origin/
generation and updates ownership and the Account-version replay barrier together.

`registered_account_version` is the version of the **current owning Account** observed at
registration/transfer. Deactivation applies only to links whose barrier is older than the
specific event version. The deactivation Account/version is retained on the ledger. Old event
replay therefore cannot revoke a home registered after reactivation or transferred into an
Account after that event. No projector/reactivation code creates or reactivates a link.

## Shared lock order and bounded transactions

All new lifecycle participants follow this order (Q1/S1/I1 must preserve it):

1. Account transaction-scoped advisory gates, distinct IDs ascending.
2. Profile lifecycle advisory gates and existing Profile row locks, IDs ascending.
3. Existing relationship locks, if needed (Profile rows still precede relationship locks).
4. Link rows, link IDs ascending.
5. Global quota/concurrency -> Profile quota -> origin slots (Q1/S1).
6. Jobs/quarantine -> attempts/audits (Q1/S1).

No session locks. Transfer takes both source/destination Account gates **before** the Profile.
Claim-only transactions must not climb from a job lock back to lifecycle gates.

Only gate-bearing transactions set LOCAL lock timeout 2s, statement timeout 3s, idle-in-transaction
limit 5s. The Rust transaction owner enforces a wall-clock 5s budget from transaction creation (activated only on a gate attempt),
including commit; PG16 has no `transaction_timeout`. Timeouts roll back (API retryable 5xx;
projector error/warning and retry). Non-gate transactions keep existing pool/DB defaults and do
not inherit that timer. Account Created/Updated/Warned do not change delivery eligibility and do
not introduce a gate. Eligibility state events do; event-store standalone compatibility writes
are wrapped atomically, while an existing outer transaction is joined.

Three revokers join the same DB-only cancellation participant:

- current-owner unlink;
- Account Deactivated event append (same transaction as link invalidation, all owned Profiles,
  including ledger-only identities; **no physical deletion dependency**);
- Account projector replay/deactivation defense, before Profile row deletion.

The Account projector reads a bounded batch, then commits **one Account per gate-bearing
transaction**, and checkpoints separately. It applies every deactivation event before deciding
the final snapshot, including a deactivate/reactivate pair in one batch. The write-side snapshot
also prevents a truncated window ending on an old deactivation from deleting a newer active
Profile/link. Keto calls happen **after commit**, never under a gate or DB transaction. Profile
restoration locks IDs in order; deactivation deletes every owned Profile, not LIMIT 1.
The existing Profile relationship advisory gate is acquired before link locks, including on
deactivation append, because the subsequent physical deletion may cascade into relationship tables.
A defensive DB trigger refuses an uncoordinated physical Profile deletion with active links.

`LinkCancellationParticipant::cancel_link_generation` is the Q1 transaction participation port:
L1 calls it with the invalidated **previous active generation**, after all relevant link locks,
before commit. Its current PostgreSQL implementation is intentionally empty because L1 has no
relay queue. Q1 replaces that DB-only implementation to cancel/delete jobs/quarantine and release
capacity on the supplied connection; it must not enqueue post-commit cleanup or wait for HTTP.
Failure must propagate to roll back the entire revocation transaction.

Send authorization is not implemented here. S1 must use the same gate/owner/link read, store its
permit and commit **before** network I/O (plan A). Revocation committed first => no new permit;
a prior successful permit may run only within its original absolute deadline. Unknown commit =>
no send. HTTP must hold **zero** DB connections/transactions/locks. L1 tests use test-owned permit
rows and P0/P1/revoker-precommit pause points; these are not production queue tables.

## Migration and recovery

Migration `20261122000200` is additive and rehearsed on empty and populated pre-L1 schemas.
Older sqlx binaries can reject the extra migration; do not claim downgrade compatibility. Restore
only a disposable DB copy for verification; prefer forward fixes, and never drop the ledger to
make an old binary start. Reconcile links/capacity and remove expired/revoked bodies before any
future relay restart after backup recovery. Physical backup retention/access controls are separate
from logical unlink; unlink does not erase historical backups.

## Emergency stop (not an owner API)

Owner unlink is **not** an operator endpoint. Stop every relay worker/operator delivery process,
or block egress at deployment infrastructure; ask the owner to unlink if possible. Until a
worker-only switch exists, stopping an integrated server may also stop its inbox. Allowlist changes
alone are **not** immediate revocation during rolling deployment. Already-authorized attempts /
bytes delivered cannot be recalled. Continued acceptance while delivery is stopped will fill quota:
monitor it. Relay workers and these operational controls are follow-up S1/B1/T1 work, not enabled
by L1.

## Acceptance evidence

All DB tests run with a real PostgreSQL 16 service; the dedicated Home links lifecycle DB CI
job rejects zero matches or ignored acceptance tests. Test names below omit their module prefix.

| Invariant | Regression test |
| --- | --- |
| Concurrent registration, active maximum one | `concurrent_registration_has_one_active_home` |
| Idempotent unlink; old ID cannot unlink new home | `unlink_is_idempotent_old_id_does_not_unlink_new_home` |
| Deactivation commit, logical deletion, all Profiles, replay barrier | `deactivation_commit_revokes_all_profiles_with_rows_remaining_and_fences_replay` |
| Authority despite projection lag; BAN and suspension expiry | `write_side_ban_is_seen_while_projection_is_active`, `suspension_uses_committed_state_and_expiry_without_unlinking` |
| P0 / revoker-precommit, stalled P1 rollback, local timeout only | `p0_and_unlink_precommit_stop_points_serialize_mock_authorization`, `p1_stalled_permit_rolls_back_and_bounds_revocation_wait`, `lifecycle_timeouts_are_transaction_local_and_plain_transactions_remain_unbounded` |
| Existing relationship gate precedes link locks | `deactivation_waits_for_existing_relationship_gate_before_locking_links` |
| Missing historical AuthAccount does not block or restore privileges | `missing_auth_history_does_not_block_projection_or_recreate_an_auth_link` |
| Populated old schema and old-binary migration refusal | `additive_migration_on_populated_main_schema_and_old_binary_refusal` (other fixtures migrate empty schemas) |
| JWT/current owner/allowlist/mode/username-less API | `home_link_api_auth_origin_modes_and_idempotency_including_non_actor_profile` |
| Held owner may inspect/unlink, but not register; no stranger access | `home_link_held_owner_can_inspect_and_unlink_but_cannot_register` |
| Transfer retains identity; prior owner loses operations | `home_link_transfer_preserves_identity_and_old_owner_cannot_list_register_or_unlink` |
| Register / unlink / actual transfer race | `home_link_register_unlink_and_real_transfer_race_keeps_only_current_owner` |
| Same-batch deactivation/reactivation; stale replay leaves new home | `home_link_projector_same_batch_deactivate_reactivate_and_stale_replay_keep_new_link` |
| FK-safe physical deletion, ledger retained, reactivation no revival | `home_link_projector_physical_delete_retains_ledger_and_reactivation_does_not_revive` |
| All three revokers serialize with P1 | `home_link_all_three_revokers_serialize_with_a_precommit_permit` |
| One Account owns multiple Profiles: projector revokes/deletes all | `home_link_projector_revokes_and_deletes_every_profile_of_an_organization` |
| Slow external Keto does not hold the gate / wait for HTTP | `home_link_account_event_commit_and_projector_do_not_wait_for_http` |
| Strict origin syntax; DNS mixed/private/rebinding; policy revalidation | `origin_normalization_and_rejection`, `home_origin_revalidates_allowlist_and_every_dns_answer_for_each_operation` |

The full S1 P2/P3/P4 send/deadline/stale-result and operator-redelivery matrix remains S1/T1
work: no production send permit, relay job, or operator-redelivery implementation exists in L1.
