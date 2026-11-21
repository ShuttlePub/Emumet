# emumet-post-relay v1 fixtures

Part of the contract in `../v1.md`. Do not edit by hand: `../tools/generate-fixtures.mjs` and
`../tools/generate-inbox-fixtures.mjs` and `../tools/generate-static-fixtures.mjs` write these files. Re-running `generate-fixtures.mjs` creates new
keys and therefore new signatures for everything under `p1/`, `p2/`, `retransmission.json`,
`signature-vectors.json` and `keys/`.

## Receiver cases (`p1/*.json`, `p2/*.json`)

```
id, phase ("P1" | "P2"), title, description,
preconditions  { link_ledger: "none" | { links: [...] } }   P1 is always "none"; P2 states the ledger
receiver       { trusted_emumet_origins, own_origin, now (RFC 3339),
                 instance_actor_documents { origin: actor document }, already_accepted_receipts [] }
request        { method, target, headers {lowercase name: value}, body }       body = exact UTF-8 text sent
               or body_synthetic { kind: "repeat", unit, bytes }                 harness builds the body
expect         { result: "accept" | "reject", status, reason, sender_class: "success" | "retry" | "permanent" }
```

The check order of `../v1.md` section 7 decides which `reason` is reported when several things are wrong;
each case is wrong in exactly one way. `p2/` cases are informative and are not part of P1 conformance.

## `retransmission.json`

One envelope sent three times. `unchanged` / `changed` list what stays and what differs; each attempt carries
the request, the receiver clock and the expected answer.

## `signature-vectors.json`

Outer-signature vectors: body, `Digest`, signing string, `Signature` header and the expected outcome
(`valid`, `digest_mismatch`, `signature_invalid`, `signing_string_mismatch`: the signature is valid for `signing_string`, but the string rebuilt from this vector's method and target is a different one). Verify with the public key named in `public_key_file`.

## `inbox/activity-reading.json`

Inbox reading and routing cases (`../v1.md` section 8): `forward`, `exclude` or `reject`, with the extracted
`activity_id` and `actor` for forwarded bodies. `body_synthetic` kinds `padding`, `nested_depth` and
`node_count` are explained in the file's `note`.

## `formats/field-formats.json`

Valid and invalid timestamps and URLs (`../v1.md` section 4.5). Each timestamp states `schema_valid` (the schema pattern) and `calendar_valid` (the day exists, checked outside the schema); each URL names the schema definition (`def`) it is checked against.

## `evidence/header-omission.json`

Input and output cases of the header-evidence algorithm (`../v1.md` section 4.3): the original header fields in the order received (`times` repeats an entry) and the expected `signature_headers`, `omitted_header_names`, `omitted_header_count` and `evidence_complete`.

## `keys/`

`*.pub.pem` public keys (SPKI PEM), `keys.json` (key id, owner, purpose) and `instance-actor-documents.json`.
Only public keys are stored. They are test keys and have no other use.
