---
facets: [invariant, vocabulary]
---

# post-relay — overview

## Goals

アカウントの住所(acct)を Emumet ドメインで提供しつつ、投稿コンテンツの送受信を
Emumet が中継する。分散思想(本体サービスは ShuttlePub)と ActivityPub 仕様
(住所=配送先)のバランスを取る中核機能。

2026-07-22 interview で確定した方針:

- **外向き**: ShuttlePub 本体から受け取った投稿に、Emumet が保持する秘密鍵で
  代理署名し、Create/Note 等として外部 ActivityPub サーバーへ配送する
- **内向き**: inbox で Create/Note 等を受信し、そのアカウントの連携先 ShuttlePub
  サーバーへ転送する(転送先は Stellar ではなく ShuttlePub 本体。features/shuttlepub-link 参照)

## Scope

- inbox の Create/Note(および Like/Announce/Delete/Update 等)ハンドラ
- 連携先 ShuttlePub サーバーへの転送機構
- ShuttlePub からの投稿受け口(内部 API)+ 代理署名 + 外部配送(OutboxActivity 記録)
- 既存の内部署名 API(POST /internal/v1/accounts/{id}/sign)との役割整理

## Acceptance criteria summary

- リモートからの Create/Note が連携先 ShuttlePub に転送される
- ShuttlePub 発の投稿が署名付きでリモート inbox に配送される(E2E で検証)

## Related

- [requirements.md](requirements.md) / [open-questions.md](open-questions.md) / [packets.md](packets.md)
- [../shuttlepub-link/overview.md](../shuttlepub-link/overview.md) — 転送先の設定元
- 既存資産: `application/src/service/activitypub/delivery.rs`, `kernel/src/entity/activitypub/outbox_activity.rs`
- 決定記録: ../../decisions/0002-account-address-on-emumet-domain.md, 0003-delegated-signing.md
