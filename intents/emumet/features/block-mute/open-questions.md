# block-mute — open questions

> See [../../clarifications/open.md](../../clarifications/open.md) for domain-level open questions.

## Open questions blocking this feature

現状なし(2026-07-22 時点)。packet 作成時に確定させる事項:

- Mute の連合扱い(ミュートは通常連合に通知しない。ローカルのみでよいか)
- Block を Event Sourcing 対象にするか、Follow 同様の純粋 CRUD にするか
