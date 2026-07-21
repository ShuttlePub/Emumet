---
facets: [invariant]
---

# media-upload — overview

## Goals

アイコン・バナー等の画像をアップロードできるようにし、Profile の icon/banner と
ActivityPub Actor の `icon`/`image` に反映する。現状 Image エンティティと Repository は
存在するが、アップロード API・ストレージ連携がなく、Actor の icon は `None` 固定
(`kernel/src/activitypub.rs`)。

## Scope

- 画像アップロード REST API(形式・サイズバリデーション)
- ストレージバックエンドとの連携
- Profile icon/banner への画像紐付け(既存 PATCH /api/v1/accounts/{id} 経由)
- Actor ドキュメントへの icon/image 反映
- リモートへの Update アクティビティ伝搬(要検討、post-relay と関連)

## Acceptance criteria summary

- 画像をアップロードすると URL が発行され、Profile に設定できる
- Actor ドキュメントの icon/image に反映される

## Related

- [requirements.md](requirements.md) / [open-questions.md](open-questions.md) / [packets.md](packets.md)
- 既存資産: `kernel/src/entity/image.rs`, `kernel/src/repository/image.rs`
