---
facets: [invariant]
---

# block-mute — overview

## Goals

ユーザーが他のアカウント(ローカル/リモート)をブロック・ミュートできるようにする。
GitHub issue #2 の未完了項目であり、**最初の packet 候補**(2026-07-22 interview で決定)。

## Scope

- ローカル/リモート両方のアカウントに対するブロック・ミュート
- ActivityPub Block アクティビティの連合(送信)と inbox での受信処理
- ブロック時の既存フォロー関係の解除(双方向)
- ブロック/ミュートの作成・一覧・解除 REST API

## Acceptance criteria summary

- ブロック/ミュートの作成・一覧・解除が REST API 経由でできる
- ブロックすると相互のフォロー関係が解除される
- リモートへのブロックは署名付き Block アクティビティとして配送される
- inbox で受信した Block/Undo(Block) がローカル状態に反映される

## Related

- [requirements.md](requirements.md) / [acceptance.md](acceptance.md) / [open-questions.md](open-questions.md) / [packets.md](packets.md)
- 既存パターン: `kernel/src/entity/follow.rs`, `application/src/service/activitypub/outbound_follow.rs`
- TODO 元: https://github.com/ShuttlePub/Emumet/issues/2
