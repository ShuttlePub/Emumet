# moderation — packets

> See [../../packets/](../../packets/) for domain-level packet list.

## Execution units

1. `moderation-role-assignment` — インスタンスロール(Admin/Moderator)割当管理 API
   (packet: `.intent-cli/issues/moderation-role-assignment/`)
2. `moderation-account-report` — 通報(AccountReport)機能
   (packet: `.intent-cli/issues/moderation-account-report/`) — depends on: moderation-role-assignment

## 未パケット化の残スコープ

- ホストモデレーション(リモートインスタンス単位の制限) — 要件整理から
