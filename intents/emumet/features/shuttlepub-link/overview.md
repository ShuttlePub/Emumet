---
facets: [vocabulary]
---

# shuttlepub-link — overview

## Goals

アカウントごとに「連携先 ShuttlePub サービス」を設定し、自分宛ての投稿の転送先を
決められるようにする。docs の StellarAccount イベント定義の後継にあたる
(2026-07-22 interview: 「メインサービスの保存」ではなく「アカウントごとの連携先
ShuttlePub サービスを設定してそこに自分宛ての投稿を流す」形)。

## Background

Stellar システムは凍結され、認可まわりは Ory で代替された(decisions/0001)。
StellarAccount イベント定義(host, client_id, access_token, refresh_token)は
コンセプトとしては有効だが、対象は Stellar ではなく ShuttlePub 本体サービス群になる。

## Scope

- アカウント ↔ 連携先 ShuttlePub サービスの紐付け(登録・更新・削除)
- post-relay の転送先解決に利用されること

## Related

- [requirements.md](requirements.md) / [open-questions.md](open-questions.md) / [packets.md](packets.md)
- [../post-relay/overview.md](../post-relay/overview.md) — 本設定の主たる利用者
- docs: https://docs.shuttle.pub/docs/emumet/data-structure (StellarAccount イベント)
