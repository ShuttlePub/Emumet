# block-mute — packets

> See [../../packets/](../../packets/) for domain-level packet list.

## Execution units

1. `block-mute-core` — ユーザーブロック/ミュート: エンティティと REST API の実装
   (packet: `.intent-cli/issues/block-mute-core/`) —
   **published: https://github.com/ShuttlePub/Emumet/issues/16** (intent-target)
2. `block-mute-federation` — ActivityPub 連合(Block アクティビティ送受信)
   (packet: `.intent-cli/issues/block-mute-federation/`) — depends on: block-mute-core
