## Goal

ユーザーブロック機能の ActivityPub 連合を実装する。ローカルからリモートへの
ブロック時に署名付き Block アクティビティを相手 inbox へ配送し、inbox での
Block / Undo(Block) 受信処理でフォロー関係を解除する。block-mute-core が追加した
ローカル基盤の上に連合レイヤーを載せる。

## Why This Slice Exists Now

block-mute-core でエンティティ・Repository・REST API が揃い、ブロックのローカル
操作が可能になった。しかし ActivityPub 連携がなければ、リモートインスタンス上の
ユーザーをブロックしても相手に通知されず、フォロー関係が残ったままになる。
inbox では現状 Follow/Accept/Undo(Follow) しか処理しておらず、Block に対応する
ハンドラがない。GitHub issue #2 の未完了項目で、2026-07-22 の interview で
block-mute-core の次スライスとして実装順序が合意された。

## Current Observed State

- inbox ハンドラー (`application/src/service/activitypub/inbox/handlers.rs`) は
  Follow, Accept, Undo(Follow) のみ処理。Block / Undo(Block) の分岐がない
- outbound は Follow のみ署名配送 (`application/src/service/activitypub/outbound_follow.rs`)。
  Block を配送するパスが存在しない
- delivery エンジン (`application/src/service/activitypub/delivery.rs`) は
  Cavage HTTP 署名で署名付きリクエストを送信する。Block 配送でも同機構が使える
- E2E テスト (`server/tests/e2e_ap_mock.rs`) には Follow 系シナリオのみ。Block 用
  シナリオがない

## Accepted Baseline You May Assume

- block-mute-core が提供する Block エンティティ・BlockRepository・REST API が使える
- outbound_follow.rs の署名配送パターンを Block に適用できる
- delivery.rs の Cavage 署名機構をそのまま Block 配送に再利用できる
- inbox ハンドラーの分岐構造を Follow パターンに倣って Block を追加できる
- Mock peer E2E テストの既存構成(ap_peer.rs)に Block シナリオを載せられる

## Target Repo / Path / Part

Repository: `ShuttlePub/Emumet`

Target paths: `application/src/service/activitypub`, `kernel/src/activitypub.rs`, `server/tests`

Target part: Block アクティビティの署名配送・inbox 受信処理・E2E テスト

## In Scope

- ローカル→リモートへのブロック時に署名付き Block アクティビティを相手 inbox へ配送
  (outbound_follow のパターン踏襲)
- ブロック解除時に Undo(Block) アクティビティを相手 inbox へ配送
- inbox で受信した Block アクティビティの処理(ローカルのフォロー関係解除)
- inbox で受信した Undo(Block) アクティビティの処理(ブロック解除反映)
- Mock peer E2E テストにブロック配送・受信シナリオを追加
- Iceshrimp E2E テストにブロックシナリオを追加(可能な範囲)

## Out Of Scope

- エンティティ・REST API 本体(block-mute-core で実装済みの前提)
- Mute の連合(ミュートは連合しない)
- Like / Announce などの他のアクティビティ型
- タイムライン表示への反映(ShuttlePub 本体側)

## Standalone Child Issue Contract

Emumet の block-mute 機能に ActivityPub 連合を追加する。block-mute-core が提供する
Block エンティティと REST API を利用し、リモートへのブロック時に署名付き Block
アクティビティを配送、受信時にフォロー関係を解除する。配送は既存の Cavage HTTP
署名機構を再利用し、inbox ハンドラーには Block / Undo(Block) の分岐を追加する。
Mute の連合は行わない(ローカル専用機能のまま)。

## Acceptance Criteria

- [ ] リモートへのブロックで署名付き Block アクティビティが相手 inbox へ配送される
- [ ] inbox で受信した Block アクティビティがフォロー解除に反映される
- [ ] Undo(Block) アクティビティでブロック解除が反映される
- [ ] Mock peer E2E テストにブロックシナリオが追加されテストが通る
- [ ] `cargo test` が通り、追加機能のテストが含まれる

## Verification

- `cargo test` (DATABASE_URL が必要なテストは既存方針に従う)
- Block → フォロー解除 → Undo(Block) のフローが E2E テストで検証されること
- `git diff --check`

## Related Links

- https://github.com/ShuttlePub/Emumet/issues/2 (元 TODO)
- intents/emumet/features/block-mute/ (overview / requirements / acceptance)
- block-mute-core (前駆 packet)

## Knowledge Maintenance

- Intent placement: intents/emumet/features/block-mute(新規ノード不要)
- ADR candidate: none
- Diagram candidate: none
- Docs update: 不要(連合プロトコルの詳細は実装コードとテストで表現)
- Closeout writeback expected: no(配送/受信で踏んだ坑は PR 説明に記録)

## Base Branch Policy

Policy: `direct-main`
Expected PR base branch: `main`

Open all child PRs against `main` directly.
