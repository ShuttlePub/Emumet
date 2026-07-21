# block-mute-core Implementation Packet

## Goal

ユーザーが他アカウント(ローカル/リモート)をブロック・ミュートできるようにするための
ドメイン基盤(エンティティ・Repository・ユースケース)と REST API を実装する。
ActivityPub 連合(Block アクティビティの配送/受信)は後続スライス block-mute-federation。

## Why

GitHub issue #2 の未完了項目であり、2026-07-22 の interview で次に取り組む機能として
決定された。現状ブロック/ミュートに関するエンティティ・API は一切存在しない。

## Scope

- Block 関係を表すエンティティと Mute 関係の表現(別テーブルか type かは実装時に
  Follow パターンに合わせて決定。判断理由は PR で記録する)
- source/destination は Follow 同様 local/remote を識別する構造
- 純粋 CRUD Repository(kernel trait + Postgres 実装 + マイグレーション)
- ブロック/ミュートの作成・一覧・解除ユースケース
- REST API(例: POST/DELETE /api/v1/accounts/{id}/block, /mute, GET 一覧)
- ブロック成立時の双方向フォロー関係解除ロジック

## Out of scope

- Block アクティビティの連合配送・inbox 受信(block-mute-federation)
- ブロック/ミュートを考慮したタイムライン側の表示制御(ShuttlePub 本体の責務)
- モデレーション(管理者による強制ブロック)との統合

## Verification

- `cargo test` で Repository・ユースケースのテストが通ること
- ブロック→フォロー解除の振る舞いテスト
- `git diff --check`

## Knowledge Maintenance (G461, optional)

- Intent placement: intents/emumet/features/block-mute(新規ノード不要)
- ADR candidate: なし(永続化方式の判断は PR 説明と closeout で記録)
- Diagram candidate: なし
- Docs update: データ構造確定後に ShuttlePub/document 側の data-structure.md へ反映(別途)
- Closeout learning: 永続化方式の最終判断と理由を closeout コメントで共有
