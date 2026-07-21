## Goal

ユーザーブロック/ミュート機能のドメイン基盤と REST API を実装する。
エンティティ・Repository・ユースケース・エンドポイントを新設し、ブロック時の
フォロー関係解除までをローカルで完結させる。連合(Block アクティビティの
配送/受信)は後続 issue で扱う。

## Why This Slice Exists Now

GitHub issue #2 の未完了項目「(ユーザー)ミュート、ブロック機能実装」であり、
2026-07-22 の intent interview で次の優先機能として決定された。
現状エンティティ・API は一切存在しないゼロからの追加で、他スライスに依存しない。

## Current Observed State

- ブロック/ミュートに関するエンティティ・テーブル・ルートは存在しない
- Follow 関係は `kernel/src/entity/follow.rs` + `FollowRepository` で local/remote を
  識別して管理されている(純粋 CRUD パターン)
- 認証済みルートは JWT middleware 経由で `Extension<AuthClaims>` を受け取る

## Accepted Baseline You May Assume

- Follow と同様の local/remote 識別子パターンが使える
- 純粋 CRUD Repository パターン(kernel trait → Postgres 実装)に従う
- ルーティング・DI は `server/src/route/account/follow.rs` と `AppModule` の既存構成に従う
- DB マイグレーションの追加方法は既存マイグレーションに従う

## Target Repo / Path / Part

Repository: `ShuttlePub/Emumet`

Target paths: `kernel/src/entity`, `kernel/src/repository`, `application/src/service`, `driver/src/database/postgres`, `server/src/route`

Target part: Block/Mute 関係エンティティ・Repository・ユースケース・REST API

## In Scope

- Block 関係および Mute 関係を表すエンティティとテーブル(マイグレーション含む)
- kernel Repository trait + Postgres 実装
- ブロック/ミュートの作成・一覧・解除ユースケースと REST API
- ブロック成立時の双方向フォロー関係解除
- 上記のテスト

## Out Of Scope

- Block アクティビティの連合配送・inbox での受信処理(後続 issue: block-mute-federation)
- タイムライン表示への反映(ShuttlePub 本体側)
- 管理者によるモデレーションとしてのブロック

## Standalone Child Issue Contract

Emumet にユーザー向けブロック/ミュート機能の基盤を追加する。ローカル/リモート両方の
アカウントを対象に、ブロック/ミュート関係の作成・一覧・解除ができる REST API を
提供し、ブロック成立時には既存のフォロー関係を双方向で解除する。永続化は既存の
純粋 CRUD Repository パターンに従い、ActivityPub 連合への通知は本 issue では扱わない。

## Acceptance Criteria

- [ ] ローカルアカウントに対してブロックの作成・一覧・解除が REST API 経由でできる
- [ ] ミュートの作成・一覧・解除が REST API 経由でできる
- [ ] リモートアカウントに対するブロック/ミュート関係をローカルに記録できる
- [ ] ブロック成立時に双方向のフォロー関係が解除される
- [ ] `cargo test` が通り、追加機能のテストが含まれる

## Verification

- `cargo test` (DATABASE_URL が必要なテストは既存方針に従う)
- ブロック→フォロー解除の振る舞いテスト
- `git diff --check`

## Related Links

- https://github.com/ShuttlePub/Emumet/issues/2 (元 TODO)
- intents/emumet/features/block-mute/ (overview / requirements / acceptance)

## Knowledge Maintenance

- Intent placement: intents/emumet/features/block-mute(新規ノード不要)
- ADR candidate: none
- Diagram candidate: none
- Docs update: データ構造確定後に ShuttlePub/document 側へ反映(別リポジトリ・別途)
- Closeout writeback expected: no(永続化方式の判断は PR 説明に記録)

## Base Branch Policy

Policy: `direct-main`
Expected PR base branch: `main`

Open all child PRs against `main` directly.
