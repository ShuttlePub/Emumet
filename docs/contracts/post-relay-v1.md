# Emumet post-relay contract v1 (revision 1)

Status: canonical text for P1. This file, `v1.schema.json`, `v1.constants.json` and `fixtures/` form one
contract. The canonical copy lives in the host repository (`ShuttlePub/host`,
`intents/shuttlepub/contracts/emumet-post-relay/`). Implementation repositories keep a snapshot that is
pinned to a host commit and checksums and is never edited on its own (see "Snapshot rule").

Words MUST, MUST NOT, SHOULD and MAY are used as in RFC 2119.

## 1. Scope

Emumet receives ActivityPub Activities on a Profile inbox, verifies them, and forwards each accepted
Activity to the ShuttlePub origin the Profile is linked to ("home"). This contract defines:

* the forwarding endpoint, the envelope and its JSON Schema (sections 3-4);
* the outer HTTP signature that authenticates Emumet to the receiver (section 5);
* what is identical and what changes when the same envelope is sent again (section 6);
* the receiver's verification procedure, split into P1 (no link ledger) and P2 and later (section 7);
* how an inbox body is read and routed (section 8) and the lifecycle rules that decide whether a
  body is sent at all (section 9);
* the bounded constants shared by documents, schema and fixtures (section 10), the fixtures (section 11)
  and the snapshot rule (section 12).

Out of scope: the ShuttlePub production receiver (storage, de-duplication), DCR, trust modes other than
"home" (mode 3), the signing API, key rotation and revocation operations (K2), object and collection
proxying, sharedInbox.

## 2. Terms

* **Emumet**: the sender. Identified by its normalized origin, for example `https://emumet.example`.
* **Receiver**: ShuttlePub, or a mock that implements section 7.
* **Instance actor**: `{origin}/ap/instance`, an `Application` actor owned by Emumet. It is not a Profile.
* **Receipt**: one Activity accepted on one Profile inbox. `receipt_id` identifies it.
* **Link**: the record that attaches a Profile to a ShuttlePub origin. `link_id` is opaque and never reused.
  `link_generation` is a positive integer that grows when the link's delivery authorization changes.
* **Original request / original signature**: the request the remote actor sent to the Emumet inbox.
* **Outer request / outer signature**: the request Emumet sends to the receiver.
* **Normalized origin**: lowercase `https` scheme and host, no userinfo, no path, query or fragment, and
  the default port omitted.

## 3. Transport

| Item | Rule |
|---|---|
| URL | `{recipient origin}` + `/internal/v1/emumet/activities` (not under `/api/v1`, not part of any generated OpenAPI) |
| Method | `POST` |
| Content-Type | `application/json` (exact media type; `charset=utf-8` is tolerated) |
| Scheme | HTTPS. Redirects MUST NOT be followed; a 3xx answer is a permanent failure |
| Body | exactly one envelope, serialized once by the sender (section 6) |
| Size | request body at most `ENVELOPE_MAX_BYTES`; decoded original body at most `INBOX_BODY_MAX_BYTES` |
| Required headers | `Host`, `Date`, `Digest`, `Content-Type`, `Signature` |

The HTTP server's own header limit is a separate ingress constraint. It is not the 16 KiB evidence
budget of section 4.3. Implementations SHOULD set it explicitly (64 KiB total is the initial value) and
answer 431 before any contract processing when it is exceeded.

## 4. Envelope

The schema `v1.schema.json` (JSON Schema 2020-12) is normative for field names, types and formats.
v1 envelopes contain exactly the fields below; an unknown field is a schema violation. A new field needs a
new `version`. A receiver MUST NOT read an unknown `version` as an older one.

| Field | Meaning |
|---|---|
| `version` | `ENVELOPE_VERSION` (1) |
| `receipt_id` | opaque id of the receipt, `OPAQUE_ID_MIN`-`OPAQUE_ID_MAX` characters of `[A-Za-z0-9_-]` |
| `received_at` | when Emumet received the Activity, RFC 3339 UTC |
| `sender.origin` | normalized Emumet origin |
| `sender.instance_actor` | `{sender.origin}/ap/instance` |
| `recipient.origin` | normalized ShuttlePub origin; fixed when the Activity is accepted and never moved to another origin |
| `recipient.profile_actor` | actor URL of the Emumet Profile that received the Activity (same origin as `sender.origin`) |
| `recipient.link_id`, `recipient.link_generation` | the link that was active when the Activity was accepted |
| `activity.id` | the Activity `id` as a string, or `null` when it is missing or not extractable. Never invented |
| `activity.actor` | the actor id |
| `activity.body_encoding` | `base64` |
| `activity.raw_body` | the received body bytes, standard Base64 with padding. Never re-serialized |
| `verification.result` | `verified` |
| `verification.scheme` | the scheme that was really used: `cavage` or `rfc9421` |
| `verification.key_id`, `verification.signer_actor` | the key and its owner that were really used, as they were when the signature was checked (not what a later fetch returns) |
| `verification.verified_at` | when Emumet verified the original signature |
| `original_request.method`, `original_request.request_target` | method (`POST`) and path-and-query of the inbox request |
| `original_request.signature_headers` | list of `{name, value}`: the retained signature-related headers (section 4.3) |
| `evidence_complete`, `omitted_header_names`, `omitted_header_count` | whether the header evidence is complete, and what was left out (section 4.3). These are top-level fields |

Emumet relays only envelopes whose `verification.signer_actor` equals `activity.actor`. A receiver MAY
treat a difference as an inconsistent envelope; P1 conformance does not require it.

### 4.1 Identity

* `receipt_id` is unique per sender: the identity is `(sender.origin, receipt_id)`.
* A remote server that re-sends the same Activity to the inbox creates a new receipt.
* De-duplication of the Activity itself has two levels when `activity.id` is not null: by Activity id, and
  by Activity id together with `recipient.profile_actor`. An envelope with a null id can only be
  de-duplicated by receipt; the same Activity received twice is not recognized as identical.
* No ordering is guaranteed. A success answer can be lost, so the same envelope can arrive more than once.

### 4.2 Original signature versus outer signature

| | Original Activity | Outer request |
|---|---|---|
| Signer | remote actor | Emumet instance actor |
| Body | the original raw bytes | the envelope bytes |
| Key | remote actor key | Emumet instance key |
| Time | fixed at reception, stored | new for every attempt |
| Location | inside the envelope | HTTP headers |

* The original signature headers MUST NOT be reused on the outer request.
* `verification.*` records the key, owner and scheme actually used. Verifying successfully is not the same
  as keeping everything needed to verify again: see `evidence_complete`.
* An old original signature is never a reason to reject the outer request.

### 4.3 Header evidence and its 16 KiB budget

`original_request.signature_headers` keeps the headers a verifier needs to check the original signature
again. Candidates are, in this order:

1. `signature` and `signature-input` (when present);
2. every header named by the `headers` parameter of a Cavage `Signature` or by the covered components of
   `Signature-Input`, in that order;
3. `digest`, `content-digest`, `date`, `host`, `content-type`, `content-length`, in the original order.

Names are lowercased. A repeated header is kept as separate entries in the original order. Rules:

* Secret-bearing headers are never kept: `authorization`, `proxy-authorization`, `cookie`, `set-cookie`,
  and any header whose name contains `token`, `secret`, `api-key`, `apikey` or `password`.
* The budget counts, in UTF-8 bytes, `len(name) + len(value)` of every retained header plus `len(name)` of
  every listed omitted name. The budget is `EVIDENCE_BUDGET_BYTES`.
* Candidates are examined in the order above. A candidate is kept when it fits in the remaining budget;
  otherwise it is omitted completely. A value is never truncated. Examination continues after an omission.
* Every candidate that is not kept (secret or over budget) is omitted. `omitted_header_count` is the number
  of omitted header fields (a repeated header counts once per field). `omitted_header_names` lists the
  distinct omitted names, lowercase, at most `OMITTED_NAMES_MAX` names; names that do not fit are only
  reflected in the count. It is therefore possible that fewer names than omitted fields are listed.
* `evidence_complete` is `true` if and only if `omitted_header_count` is `0` (then `omitted_header_names`
  is empty). Otherwise it is `false` and the count is at least 1. The schema enforces this.
* An envelope with incomplete evidence is still valid and is still forwarded.

At most `EVIDENCE_MAX_HEADERS` headers are listed, each name at most `EVIDENCE_HEADER_NAME_MAX` characters
and each value at most `EVIDENCE_HEADER_VALUE_MAX` printable ASCII characters (`0x20`-`0x7e`).

### 4.4 Checks outside the schema

JSON Schema cannot express these; a receiver MUST perform them:

1. The request body is at most `ENVELOPE_MAX_BYTES` bytes (413 `envelope_too_large`).
2. `activity.raw_body` decodes to at most `INBOX_BODY_MAX_BYTES` bytes (413 `activity_too_large`).
   The Base64 length limit in the schema (`RAW_BODY_BASE64_MAX_CHARS`) is slightly larger than the exact limit.
3. The evidence budget of section 4.3 holds: the sum of `len(name) + len(value)` over
   `signature_headers` plus the lengths of `omitted_header_names` is at most `EVIDENCE_BUDGET_BYTES`
   (400 `evidence_budget_exceeded`).
4. The cross-field rules of section 7 step 5.

## 5. Outer signature

* Scheme: Cavage (draft-cavage-http-signatures), `algorithm="rsa-sha256"`: RSASSA-PKCS1-v1_5 with SHA-256,
  RSA-2048 keys. RFC 9421 is not used for the outer request in P1.
* Signed components, exactly and in this order: `(request-target) host date digest`. A signature that
  covers fewer or other components is not accepted.
* `Signature: keyId="{key id}",algorithm="rsa-sha256",headers="(request-target) host date digest",signature="{base64}"`
* Signing string: the four lines `(request-target): post /internal/v1/emumet/activities`,
  `host: {host}`, `date: {date}`, `digest: {digest}` joined with LF, without a trailing LF.
* `Digest: SHA-256={base64 of the SHA-256 of the exact request body bytes}`.
* `Date`: IMF-fixdate (`Sat, 21 Nov 2026 09:00:00 GMT`), new for every attempt. Allowed skew between `Date`
  and the receiver clock: `DATE_TOLERANCE_SECONDS`, inclusive, in both directions.
* `Host`: the authority of the receiver's configured public origin. It is not taken from forwarded headers.
* `keyId`: `{sender.origin}/ap/instance#key-{opaque}`. Key ids are never reused. The instance actor document
  lists its keys in `publicKey` (an array: several keys may be published at the same time, for example
  during a later rotation). The receiver selects the key by `keyId`.
* The public key is fetched from the instance actor document at the configured Emumet origin, over HTTPS,
  without redirects and with the usual SSRF protection. The `keyId` is never fetched before its origin is
  checked against the configured Emumet origins.
* Key rotation and revocation (K2) are later work. This contract only requires receivers to honour
  `publicKey` arrays and `keyId` selection so that a rotation does not need a new contract.

## 6. Retransmission: what changes and what does not

The sender serializes the envelope **once** and stores those bytes. Every automatic retry and every manual
redelivery sends the same bytes.

Unchanged (byte for byte): the request body, therefore `Digest`; `receipt_id`; `received_at`; `sender`;
`recipient` (origin, Profile, link id, link generation as fixed at acceptance); `activity.*`;
`verification.*`; `original_request.*`; the evidence fields; the request target and destination origin.

Changed on each attempt: the `Date` header and the `Signature` header (it signs the new `Date`).

Consequences for the receiver:

* `received_at` can be up to about 33 days old (72 hours of automatic retry, then 30 days of quarantine,
  then a manual redelivery). **A stale `received_at` alone MUST NOT cause rejection.** Freshness is judged
  only by the outer `Date` and signature.
* The same `(sender.origin, receipt_id)` can arrive again. A durable acceptance is acknowledged with `202`;
  a repeat is acknowledged with `200`. Both are success.
* A send that was authorized just before a link was removed can still arrive afterwards (section 9). In P1
  the receiver cannot tell and accepts it.

The sender classifies answers like this:

| Answer | Sender action |
|---|---|
| 2xx | success |
| 401, 408, 429, 5xx, timeout, connection or DNS failure, signing temporarily impossible | retry (30 s base, doubling, cap 1 h, equal jitter 50-100 % of the base, Retry-After as a lower bound capped by 1 h and the remaining window) |
| other 4xx | permanent failure |
| 3xx | permanent failure, no redirect is followed |
| invalid URL, SSRF violation | not sent, permanent failure |
| removed link, generation mismatch, withdrawn account | cancel and delete the body |

Automatic delivery stops `AUTO_RETRY_WINDOW_HOURS` hours after `received_at`. Permanent failures and the
72-hour cut-off keep the body in a quarantine for `QUARANTINE_RETENTION_DAYS` days (section 9).

## 7. Receiver verification

Order matters: the cheap checks come first and nothing in the body is parsed before the sender is
authenticated. The receiver is configured with its own public origin and the list of Emumet origins it
trusts. In P1 that configuration is the only trust input; the receiver holds no link ledger.

| Step | Check | Failure (status `reason`) |
|---|---|---|
| 0 | `Content-Type` is `application/json` | 415 `unsupported_media_type` |
| 0 | body size at most `ENVELOPE_MAX_BYTES` | 413 `envelope_too_large` |
| 1 | a `Signature` header exists and parses | 401 `signature_missing` / `signature_malformed` |
| 1 | `algorithm="rsa-sha256"` and `headers` is exactly `(request-target) host date digest` | 401 `signature_unsupported` |
| 1 | the origin of `keyId` equals a configured Emumet origin (normalized, exact). Nothing is fetched before this | 401 `untrusted_key_origin` |
| 2 | the instance actor document of that origin has `id` = `{origin}/ap/instance`; `keyId` is one of its `publicKey` ids; that key's `owner` is the instance actor. A Profile key of the same origin is not acceptable | 401 `unknown_key` / `key_owner_mismatch` |
| 3 | `Host` equals the authority of the configured own origin | 401 `host_mismatch` |
| 3 | `Digest` exists and equals the SHA-256 of the received body bytes | 401 `digest_missing` / `digest_mismatch` |
| 3 | `Date` parses and is within `DATE_TOLERANCE_SECONDS` of the receiver clock | 401 `date_invalid` / `date_skew` |
| 3 | the signature verifies over the signing string with the selected key | 401 `signature_invalid` |
| 4 | the body is UTF-8 JSON. An integer `version` other than 1 is `unsupported_version`. Then the envelope validates against `v1.schema.json` and passes section 4.4 | 400 `invalid_json` / `unsupported_version` / `schema_violation` / `evidence_budget_exceeded`; 413 `activity_too_large` |
| 5 | `sender.origin` equals the origin of `keyId` | 403 `sender_origin_mismatch` |
| 5 | `sender.instance_actor` equals the instance actor confirmed in step 2 | 403 `instance_actor_mismatch` |
| 5 | the origin of `recipient.profile_actor` equals `sender.origin` | 403 `profile_origin_mismatch` |
| 5 | `recipient.origin` equals the receiver's configured own origin. Unverified `Forwarded` / `X-Forwarded-*` headers are never used | 403 `recipient_origin_mismatch` |
| 6 | (P1) `link_id` and `link_generation` are the trusted Emumet's own claim. **No link ledger is read, no maximum generation is recorded, and an older generation is not rejected.** | - |
| 7 | persist durably, then answer | 202 `accepted` (new) / 200 `duplicate` |

The receiver MAY apply its own overload answers: 429 or 503 with `Retry-After`; and 5xx on storage failure.
The actor of the original Activity is not required to be on the Emumet origin.

Sender classes: 401 and 429/503/5xx are retried, 400, 403, 413 and 415 are permanent (section 6).

### 7.1 P2 and later (not part of P1 conformance)

When the receiver can obtain trusted link state, it additionally checks that the Profile, link id, link
generation and the link's active state match. How that state is obtained, its delay and its revocation are
decided in P2. `fixtures/p2/` shows the intended checks; each file states the ledger it needs, and the
reason codes there are provisional. P1 fixtures never depend on any of this and contain no "older
generation" rejection.

## 8. Reading an inbox Activity

Applies to Emumet's Profile inbox. Forwarding does not require a typed `Activity` model; the minimum read
is defined here.

### 8.1 Entry checks

| Condition | Answer |
|---|---|
| body larger than `INBOX_BODY_MAX_BYTES` | 413 `body_too_large` |
| not UTF-8 JSON (a byte order mark counts as invalid), not an object | 400 `invalid_json` / `not_object` |
| a duplicate key in any object at any depth (keys compared after JSON unescaping) | 400 `duplicate_key` |
| nesting deeper than `JSON_MAX_DEPTH` (root object = depth 1), or more than `JSON_MAX_NODES` JSON values (every value counts; keys do not) | 400 `too_deep` / `too_many_nodes` |
| `Content-Type` other than `application/activity+json` or `application/ld+json` | 400 `invalid_content_type` |
| media types are case-insensitive. Accepted parameters: `charset=utf-8` on both; on `application/ld+json` also `profile="https://www.w3.org/ns/activitystreams"` (or no profile). Any other parameter | 400 `invalid_content_type` |
| `type` missing, not a non-empty string or non-empty array of non-empty strings, or an array longer than `TYPE_ARRAY_MAX` | 400 `invalid_type` |
| actor missing or ambiguous (authentication is never loosened) | 400 `actor_missing_or_ambiguous` |
| ambiguous relation operation (section 8.3) | 400 `ambiguous_relation`, relations unchanged |
| bad signature, signer does not match the actor | 401 (unchanged existing behavior) |
| Profile missing, not an actor, owner not active | 404 (unchanged) |
| storage failure | retryable 5xx |
| per-Profile rate / count / bytes exceeded | 429 with `Retry-After` |
| global or audit limit exceeded, physical capacity short or unknown | 503 with `Retry-After` |

Other forms that are accepted for forwarding: `to`/`cc` given as strings, an actor given as an object, a
missing Activity id, an `object` that is an array or missing, unknown extension types. External JSON-LD
contexts are never fetched.

### 8.2 Extraction

* `actor`: a URL string, an object with a string `id`, or an array (at most `ACTOR_ARRAY_MAX` elements)
  whose elements all resolve to the same single id. Anything else is `actor_missing_or_ambiguous`.
* `id`: the string value, otherwise `null`. A missing or non-string `id` never blocks forwarding of a
  non-Follow/Block Activity.
* `type`: the recognized ActivityStreams terms are the bare term (`Follow`), the compact form (`as:Follow`)
  and the full IRI (`https://www.w3.org/ns/activitystreams#Follow`). Everything else is an extension type.

### 8.3 Routing

Let R be the set of recognized terms among `{Follow, Block, Undo, Accept, Reject}` found in `type`.

1. `|R| >= 2` (for example `Follow`+`Block`, or `Undo`+`Follow`): 400 `ambiguous_relation`, nothing changes.
2. `R = {Follow}` or `{Block}`: **not forwarded**; handled by the existing relation processing. An extension
   type listed next to it does not change this.
3. `R = {Undo}`, `{Accept}` or `{Reject}`: look at `object`.
   * A reference is *certain* when it is an embedded object whose recognized type is `Follow` or `Block`,
     or when it is an id (a string or an object `id`) of a Follow/Block activity already stored for this
     Profile. Nothing is fetched to decide. A prior-Undo guard marker is not a stored Follow: it never makes
     a reference certain.
   * The body is **not forwarded** only when `object` yields at least one reference and **every** reference is
     certain. A missing `object`, an empty array, a URL that is not known, or any mixed array means the
     reference is unknown and the **whole body is forwarded**. Extension types next to Undo/Accept/Reject do
     not change this.
4. Anything else is forwarded.

Excluded bodies are not relayed and get no envelope; the relation processing keeps its present behavior
(including the early-Undo guard). Forwarding does not replace the existing duplicate protection of relation
updates, and a stored receipt is not proof that a forward happened.

## 9. Lifecycle rules that decide whether a body is sent

These are decisions, shared by the units that implement them.

1. **Removal of a link stops sending.** After a link is removed, no new send and no new retry is authorized.
   A send that was authorized just before the removal may still reach the receiver. "Authorized" means the
   transaction that gives the job its send permission has committed. That commit time starts an absolute
   `SEND_DEADLINE_SECONDS` limit (never longer than the remaining 72 hours, or the quarantine expiry for a
   manual redelivery), and no database transaction, connection or lock is held while the HTTP request runs.
   A timeout ends the sending; it is not a guarantee that bytes already delivered can be withdrawn.
2. **Unknown references are forwarded** (section 8.3). Only Follow, Block and Undo/Accept/Reject that
   certainly refer to a known Follow/Block are excluded.
3. **Quarantine for 30 days.** Only a permanent failure and the 72-hour cut-off keep the envelope for
   `QUARANTINE_RETENTION_DAYS` days (counted from the terminal event), with a body-less summary record for the
   same period. An operator can redeliver it manually, with the same envelope, receipt and destination and a
   fresh outer Date and signature. Success and cancellation delete the job, the attempts and the body and
   leave no per-delivery row. This is a limited exception to ADR 0003 ("post content is not stored in
   Emumet"); quarantined bodies are not used for storage, search or ordinary delivery.
4. **Profile transfer keeps the link**: link id, origin and generation are unchanged and only the new owner
   can operate the link. Jobs continue to the same destination; quarantined bodies keep their original expiry.
5. **Re-activation does not revive a link.** Withdrawal of the account removes the link (an event / state
   transition, not the physical row deletion) and increases its generation. A re-activated account must
   register the link again, which creates a new link id.
6. Suspension and ban hold sending until resumed or until the original 72-hour / 30-day limits expire; they
   never extend a limit and manual redelivery is not allowed while held.
7. The grace period before the physical deletion of an account and the deletion itself are a separate topic.
   The removal of the link does not wait for them.

The removal paths are the owner's unlink API, the account withdrawal command and the account projector's
withdrawal handling.

## 10. Bounded constants

Documents, `v1.schema.json`, `v1.constants.json` and the fixtures MUST agree with this table. A test
compares them.

| Name | Value | Meaning |
|---|---|---|
| `ENVELOPE_VERSION` | 1 | value of `version` |
| `INBOX_BODY_MAX_BYTES` | 1048576 | largest original body (1 MiB) |
| `ENVELOPE_MAX_BYTES` | 2097152 | largest outer request body (2 MiB) |
| `RAW_BODY_BASE64_MAX_CHARS` | 1398104 | schema bound for `raw_body` (Base64 of 1 MiB, rounded up) |
| `EVIDENCE_BUDGET_BYTES` | 16384 | retained headers plus omission names (16 KiB) |
| `EVIDENCE_MAX_HEADERS` | 64 | entries in `signature_headers` |
| `EVIDENCE_HEADER_NAME_MAX` | 128 | characters in a header name |
| `EVIDENCE_HEADER_VALUE_MAX` | 16384 | characters in a header value |
| `OMITTED_NAMES_MAX` | 32 | names listed in `omitted_header_names` |
| `REQUEST_TARGET_MAX` | 2048 | characters in `request_target` |
| `URL_MAX` | 2048 | characters in an id, actor or key URL |
| `ORIGIN_MAX` | 255 | characters in an origin |
| `OPAQUE_ID_MIN` | 8 | shortest `receipt_id` / `link_id` |
| `OPAQUE_ID_MAX` | 128 | longest `receipt_id` / `link_id` |
| `LINK_GENERATION_MAX` | 9007199254740991 | largest `link_generation` (2^53 - 1) |
| `JSON_MAX_DEPTH` | 32 | JSON nesting depth of an inbox body (root object = 1) |
| `JSON_MAX_NODES` | 100000 | JSON values in an inbox body |
| `TYPE_ARRAY_MAX` | 16 | entries in `type` |
| `ACTOR_ARRAY_MAX` | 16 | entries in an `actor` array |
| `DATE_TOLERANCE_SECONDS` | 300 | outer `Date` skew, inclusive |
| `SEND_DEADLINE_SECONDS` | 10 | absolute limit from the send permission |
| `RETRY_BASE_SECONDS` | 30 | first retry base; doubles per failure |
| `RETRY_CAP_SECONDS` | 3600 | retry base cap |
| `RETRY_JITTER_MIN_PERCENT` | 50 | equal jitter lower bound of the base |
| `AUTO_RETRY_WINDOW_HOURS` | 72 | automatic delivery window from `received_at` |
| `QUARANTINE_RETENTION_DAYS` | 30 | quarantine and summary retention |

## 11. Fixtures

Everything is under `fixtures/`. See `fixtures/README.md` for the file formats.

* `p1/`: P1 receiver cases. They hold no link ledger and none of them rejects for an old generation.
  Includes unknown Emumet, key id of another origin, Profile key, owner mismatch, wrong signature, origin
  mismatches, Host / Digest / Date mismatches, unknown version, schema violations, oversize, a normal
  acceptance without a ledger, and a normal acceptance with a `received_at` about 33 days old.
* `retransmission.json`: one envelope sent three times; states what is unchanged and what changes.
* `signature-vectors.json`: outer-signature vectors (valid and invalid) with signing strings.
* `inbox/activity-reading.json`: the reading and routing rules of section 8.
* `p2/`: informative cases that need a link ledger; each states its precondition. They are separate from P1.
* `keys/`: public keys and instance actor documents. Private keys are not stored: they were discarded after
  signing. `tools/generate-fixtures.mjs` regenerates everything with new keys.

## 12. Snapshot rule

The host copy is the only place where this contract is edited. A repository that needs it keeps a snapshot
with the host repository, the host commit, the contract version and revision, and the SHA-256 of every
copied file written next to it. Snapshots are not edited independently. A change to this contract produces a
new host commit (and a new revision, or a new version when the change is incompatible) and then new
snapshots.
