# Registration enforcement preparation (E15d, part 1)

> Historical preparation scope. Part 2 supersedes deployment details: see [current runbook](registration-enforcement.md).

**Not production enforcement yet.** This preparatory change does not modify
`ory/kratos/kratos.yml`, compose, the public router or OpenAPI. Existing Kratos
registrations remain unchanged. Issue #81 and D15 remain incomplete until the
attestation/ingress/production-wiring and real-Kratos acceptance slice lands.

## Internal listener (opt-in)

| Variable | Default / meaning |
|---|---|
| `REGISTRATION_ENFORCEMENT_ENABLED` | unset / `false`: no listener and no maintenance worker. Only `true` enables them |
| `REGISTRATION_HOOK_LISTEN` | `127.0.0.1:8081`; never expose this on the public ingress |
| `REGISTRATION_HOOK_ALLOWED_PEERS` | `127.0.0.1,::1`; exact transport peer IPs, **not** forwarded headers or CIDRs |
| `REGISTRATION_HOOK_SECRET` / `REGISTRATION_HOOK_SECRET_FILE` | exactly one required when enabled; at least 32 visible ASCII bytes; use a random secret and a restricted file |
| `REGISTRATION_PROTECTION_POLL_SECS` | 30 seconds; positive, strictly less than half `REGISTRATION_PROTECTION_PROBE_MAX_AGE_SECS` (default 600 seconds) |

The private `POST /registration/pre-persist` expects
`Authorization: Bearer <secret>` and this allowlisted JSON context:

```json
{
  "flow_id": "89c3e9c6-9a10-4e1c-ae02-a04b53cc4166",
  "flow_type": "api",
  "email": "registrant@example.com",
  "email_proof": null,
  "bot_token": null
}
```

`flow_type` is `api` or `browser`. Unknown fields (including self-declared
override, credentials, cookies and sessions), malformed or oversized bodies,
bad authentication and disallowed transport peers receive structured 403.
The full request/body/database/provider deadline is 5 seconds. The only hook
responses are empty 204 (allow) and Kratos validation-message 403 (deny).
Timeout/internal failure is a temporary 403, not a retry-inducing 5xx. The
listener does not log request bodies, PII, token/proof, secrets or provider errors.

The Axum listener is HTTP on a private/loopback hop. **Do not deploy it across
hosts without a TLS/mTLS terminator and network policy.** Kratos-to-terminator
TLS, immutable hook configuration and admission/ingress attestation belong to
part 2; this opt-in alone is not a production security boundary. No default
secret, unauthenticated control endpoint or test provider URL is added.

## Final-submit use case

The internal use case synchronizes current generation/freshness into E15a on
every call. Closed/stopped/temporary rejection cannot use an old flow or cached
verdict to bypass the decision. Each effective protection is required, including
during override. Email proof is bound to flow/email/current generation and
atomically consumed by E15c (same-binding transport retries reuse only the
verified fact). BOT tokens are verified by the Siteverify adapter, never trusted
from the client.

A rejected user token never downgrades provider health. A provider failure
updates readiness; a still-healthy protection may satisfy the final decision.
After provider I/O the use case checks the gate again: a new closure is honored
and a newly effective but not yet verified protection requires retry. No
atomicity is claimed between that final evaluation and Kratos persistence.
Turnstile's single-use semantics can cause a lost successful hook response to
require a fresh token; an old gate allow verdict is never replayed.

Login and authenticated Account creation are untouched. Proxy-aware public
email issuance rate limiting and frontend CSRF-cookie forwarding remain
integration concerns for part 2; the internal listener never trusts source
headers to manufacture a user address.

## Periodic evidence and concurrency

Enabled maintenance checks email connection/MAIL FROM, Turnstile's interpreted
negative probe, and delivery to the **configured fixed canary address only**.
It never uses an arbitrary registrant address. Cycle deadline is 20 seconds;
default polling is 30 seconds, well within the 10-minute probe freshness budget.
Readiness is synchronized even when there is no new provider result, so expired
successes cannot survive worker delay or restart.

Each generation retains a strict initial bootstrap:

- email: the administrator's input-less verification, which really sends to the fixed canary and requires SMTP DATA acceptance;
- BOT: the administrator's input-less verification; Siteverify rejects the deliberately invalid probe token as `invalid-input-response` (the secret is valid), which is the business success.

After bootstrap, successful fixed-canary **DATA acceptance** and interpreted
Siteverify probe responses renew the business TTL (default 24 hours), including
after TTL expiry or a prolonged outage. MAIL FROM alone cannot renew email
business evidence. A different generation must bootstrap again. Periodic mail
uses a random code but creates no user challenge or proof; its purpose is
delivery/API revalidation, not a new ownership claim.

This implements the parent clarification `question-273f21f1ce70800ab974ed89f9c8f6bd`:
D15 requires verification API evidence, not perpetual mailbox roundtrips.
SMTP failure-stage and evidence-time rules remain intact. Successful exchanges
carry their **start time** as a conservative evidence lower bound; failures carry
the time observed. A slow success started before a newer fault cannot clear it.
Confirming an old code cannot clear a newer SMTP failure.

A transaction-scoped advisory lock excludes overlapping workers. The singleton
`registration_protection_schedule` persists the next due time, excluding
sequential replicas in the same period too. The outer transaction holds only the
schedule lock; readiness and gate/event changes use short independent
transactions. Cancellation/process death rolls back the reservation and releases
the lock. Delivery is not exactly-once across a crash after SMTP acceptance:
a subsequent worker may revalidate again, but no two maintenance cycles run
concurrently and unchanged health does not multiply notifications.

Provider recovery does not clear a configuration-loss latch or administrator
closure. E15a owns transitions/events; E15w delivery failures do not affect the
gate. No Webhook delivery implementation changes are part of this slice.

## Verification and remaining acceptance

- `cargo test -p server registration_guard::`: strict context/auth/peers,
  204/403, deadline and secret-safe configuration.
- With a real `DATABASE_URL`:
  `cargo test -p application registration_protection::tests -- --nocapture`:
  proof/override/final-state behavior, downgrade versus user rejection,
  persistent/concurrent cycle exclusion, TTL renewal and configuration latch.
  CI runs this explicitly against PostgreSQL and rejects an all-skipped result.
- Existing driver real-DB and protection adapter regressions remain in CI.

These tests use fake providers and do **not** substitute for part 2's real Kratos
browser/API/pre-existing-flow matrix, admin API + identity DB persistence
assertions, configuration attestation, unknown-replica routing denial,
notification/Webhook/unread integration and login/Account regressions.
