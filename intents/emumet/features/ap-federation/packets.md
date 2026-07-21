# ap-federation — packets

> See [../../packets/](../../packets/) for domain-level packet list.

## Execution units

1. `mastodon-e2e-completion` — Mastodon 連携 E2E テストの完成
   (packet: `.intent-cli/issues/mastodon-e2e-completion/`)

## 関連 packet(他 feature 管轄)

- `block-mute-federation` — Block アクティビティ連合([../block-mute/packets.md](../block-mute/packets.md))
- 投稿系アクティビティ(Create/Note 等)の送受信・転送は post-relay feature 側で
  open question 解消後にパケット化予定
