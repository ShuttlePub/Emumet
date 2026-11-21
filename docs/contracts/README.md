# Contracts

## post-relay v1 (`post-relay-v1*`)

The contract for forwarding an ActivityPub Activity that Emumet received on a Profile inbox to the
ShuttlePub origin the Profile is linked to: the envelope and its JSON Schema, the outer HTTP signature,
what changes and what does not when an envelope is sent again, the receiver's verification procedure,
how an inbox body is read and routed, and the bounded constants.

**These files are a snapshot. Do not edit them here.** The canonical copy is in the host repository
(`ShuttlePub/host`, `intents/shuttlepub/contracts/emumet-post-relay/`). `post-relay-v1.snapshot.json`
names the host commit, the contract version and revision, and the SHA-256 of every file. To change the
contract: change the host copy, commit it, then copy the files again and update the snapshot.

| File | Canonical file in the host repository |
|---|---|
| `post-relay-v1.md` | `v1.md` |
| `post-relay-v1.schema.json` | `v1.schema.json` |
| `post-relay-v1.constants.json` | `v1.constants.json` |
| `post-relay-v1-fixtures/` | `fixtures/` |
| `post-relay-v1.snapshot.json` | (written when the snapshot is taken) |

`server/tests/post_relay_contract.rs` runs in CI. It checks the snapshot checksums, that the constants
in the document, the schema and the fixtures agree, the envelope schema, a reference receiver that holds no
link ledger (every P1 fixture), the signature vectors, the retransmission invariants, and a reference
reader for inbox bodies. The reference code is test-only; it is not the implementation. Run it with
`nix develop -c cargo test -p server --test post_relay_contract`.

The fixtures contain only public test keys. Regenerating them (in the host repository) creates new keys.
