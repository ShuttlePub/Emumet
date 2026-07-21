---
facets: [invariant]
---

# ap-federation — overview

## Goals

ActivityPub 連合との相互運用を提供する。**基盤は実装済み**で、残りは投稿系
アクティビティと Actor プロフィールの反映(post-relay / media-upload と連携)。

## 実装済みスコープ

- WebFinger(`/.well-known/webfinger`)、Actor ドキュメント(Person + publicKey)
- Inbox(Follow / Accept / Undo(Follow) のみ)・ Outbox(カーソルページネーション)
- Followers / Following コレクション
- Follow の外向き配送(署名付き)・リモート Actor 解決キャッシュ
- HTTP Signature(Cavage 検証 / Cavage+RFC9421 署名)・ SSRF 対策
- E2E: Mock peer (S1-S6)、Iceshrimp (S7-S9)

## 残スコープ

- Create/Note 等の投稿系アクティビティ → [../post-relay/overview.md](../post-relay/overview.md)
- Actor の icon/image 反映 → [../media-upload/overview.md](../media-upload/overview.md)
- Block アクティビティ → [../block-mute/overview.md](../block-mute/overview.md)
- Mastodon E2E の完成(compose とテスト骨格は存在、テストケース未完成)
  — `compose.ap-e2e.yml`, `server/tests/e2e_ap_mastodon.rs`

## Related

- [packets.md](packets.md)
- コード: `server/src/route/activitypub/`, `application/src/service/activitypub/`, `kernel/src/activitypub.rs`
