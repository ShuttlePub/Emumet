# block-mute-federation Implementation Packet

## Goal

ユーザーがリモートアカウントをブロックしたとき、署名付き Block アクティビティを
相手の inbox へ配送し、受信側でも Block / Undo(Block) を処理してフォロー関係を
解除できるようにする。block-mute-core が追加したローカルのブロック基盤の上に
ActivityPub 連合を載せるスライス。

## Why

block-mute-core でローカルのエンティティ・REST API が揃ったので、次に ActivityPub
連合を実装する必要がある。inbox では現状 Follow/Accept/Undo(Follow) しか処理して
おらず、Block と Undo(Block) のハンドリングがない。outbound も Follow への署名
配送のみで、Block の配送パスが未実装。GitHub issue #2 の未完了項目で、2026-07-22 の
interview でこの順序で実装することが合意された。

## Scope

- ローカル→リモートへのブロック時に、署名付き Block アクティビティを相手の inbox
  へ配送する(outbound_follow のパターンを踏襲)
- ブロック解除時に Undo(Block) アクティビティを相手の inbox へ配送する
- inbox で受信した Block アクティビティを処理し、ローカルのフォロー関係を解除する
- inbox で受信した Undo(Block) アクティビティを処理し、ブロック解除を反映する
- Mock peer E2E テストにブロック配送・受信シナリオを追加する
- Iceshrimp E2E テストにブロックシナリオを追加する(可能な範囲で)

## Out of scope

- エンティティ・REST API 本体(block-mute-core で実装済みの前提)
- Mute の連合(ミュートは連合しない。ローカルのみの機能)
- Like / Announce などの他のアクティビティ型
- タイムライン表示への反映(ShuttlePub 本体の責務)

## Verification

- `cargo test` で inbox ハンドラー・配送ロジックのテストが通ること
- Block → フォロー解除 → Undo(Block) → 解除解除の一連のフローがテストで検証されること
- E2E テスト(mock peer) で署名付き配送が実際に到達すること
- `git diff --check`

## Knowledge Maintenance (G461, optional)

- Intent placement: intents/emumet/features/block-mute(新規ノード不要)
- ADR candidate: なし
- Diagram candidate: なし
- Docs update: 不要(連合プロトコルの詳細は実装コードとテストで表現)
- Closeout learning: Block アクティビティの配送/受信で踏んだ坑を closeout コメントで共有
