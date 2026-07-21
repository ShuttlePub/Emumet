## Goal

アカウント通報(AccountReport)機能のドメイン基盤と REST API を実装する。
ユーザーが通報を作成でき、モデレーター/Admin が一覧・詳細参照・クローズを行えるようにする。
docs(data-structure.md) に定義された account_report イベント群に対応する
エンティティ・EventStore・ReadModel・ユースケース・エンドポイントを新設する。

## Why This Slice Exists Now

docs に account_report_created / updated / closed のイベント定義が既にあるが、
実際のエンティティ・テーブル・API は一切存在しない。2026-07-22 の interview で
「アドミン達がモデレーションをちゃんとできる状態」に向けたスコープとして決定された。
通報受付・一覧・クローズはモデレーション運用に不可欠な最低限機能。

## Current Observed State

- AccountReport 関連のエンティティ・テーブル・ルートは存在しない
- docs(data-structure.md) に account_report_created / updated / closed イベント定義あり
- Account / AuthAccount / Profile / Metadata は既に Event Sourcing + CQRS で実装済み
- 権限モデル: InstanceRole(Admin, Moderator)、PermissionChecker/PermissionWriter trait
  (`kernel/src/permission.rs`)
- ロール割当 API は moderation-role-assignment で別途実装予定

## Accepted Baseline You May Assume

- kernel interface traits: `EventStore` / `ReadModel` のパターンに従う
  (`kernel/src/event_store/`, `kernel/src/read_model/`)
- adapter 層: `CommandProcessor` / `QueryProcessor` の blanket impl で組み立てる
  (`adapter/src/`)
- driver 層: Postgres による `EventStore` / `ReadModel` の具体実装
  (`driver/src/database/postgres/`)
- application 層: use case + event applier パターン
  (`application/src/service/account/`, `server/src/applier/`)
- 認証済みルートは JWT middleware 経由で `Extension<AuthClaims>` を受け取る
- ルーティング・DI は `AppModule` の既存構成に従う
- DB マイグレーションの追加方法は既存マイグレーションに従う
- moderator 権限チェックは `moderation-role-assignment` 実装後に利用可能

## Target Repo / Path / Part

Repository: `ShuttlePub/Emumet`

Target paths: `kernel/src/entity`, `kernel/src/event_store`, `kernel/src/read_model`, `application/src/service`, `driver/src/database/postgres`, `server/src/route`

Target part: AccountReport エンティティ・EventStore・ReadModel・ユースケース・REST API

## In Scope

- AccountReport エンティティと Event enum(Created / Updated / Closed)
- AccountReportEventStore kernel trait + Postgres 実装
- AccountReportReadModel kernel trait + Postgres 実装
- account_report_events / account_reports テーブル(マイグレーション含む)
- 通報作成ユースケースと REST API: POST /api/v1/accounts/{id}/report
- 通報一覧 API: GET /api/v1/admin/reports (Moderator / Admin のみ)
- 通報詳細 API: GET /api/v1/admin/reports/{id} (Moderator / Admin のみ)
- 通報クローズ API: POST /api/v1/admin/reports/{id}/close (Moderator / Admin のみ)
- 権限チェック(非モデレーターは管理 API を利用不可)
- 上記のテスト

## Out Of Scope

- 通報に対するモデレーションアクション実行の自動化
- リモートインスタンスへの通報連合(ActivityPub Flag アクティビティの配送/受信)
- ホスト(リモートインスタンス)単位のモデレーション

## Standalone Child Issue Contract

Emumet にアカウント通報(AccountReport)機能を追加する。ユーザーが通報を作成でき、
Moderator / Admin ロール保持者が通報の一覧・詳細参照・クローズを行える REST API を
提供する。永続化は既存の CQRS パターン(EventStore + ReadModel)に従い、
docs の account_report イベント定義に準拠する。ActivityPub 連合や自動化アクションは
本 issue では扱わない。

## Acceptance Criteria

- [ ] ユーザーがアカウントを通報できる (POST /api/v1/accounts/{id}/report)
- [ ] Moderator ロール保持者が通報一覧を参照できる (GET /api/v1/admin/reports)
- [ ] Moderator ロール保持者が通報詳細を参照できる (GET /api/v1/admin/reports/{id})
- [ ] Moderator ロール保持者が通報を close_reason 付きでクローズできる (POST /api/v1/admin/reports/{id}/close)
- [ ] 非モデレーターが管理 API を利用できない (403)
- [ ] `cargo test` が通り、追加機能のテストが含まれる

## Verification

- `cargo test` (DATABASE_URL が必要なテストは既存方針に従う)
- 権限チェックテスト: 非モデレーターが管理 API を叩いて 403 が返ること
- `git diff --check`

## Related Links

- intents/emumet/features/moderation/ (overview / requirements / packets)
- https://docs.shuttle.pub/docs/emumet/data-structure (AccountReport イベント定義)

## Knowledge Maintenance

- Intent placement: intents/emumet/features/moderation(新規ノード不要)
- ADR candidate: none
- Diagram candidate: none
- Docs update: AccountReport イベント定義を実装に合わせて ShuttlePub/document 側へ反映(別リポジトリ・別途)
- Closeout writeback expected: no(永続化方式と close_reason スキーマの判断は PR 説明に記録)

## Base Branch Policy

Policy: `direct-main`
Expected PR base branch: `main`

Open all child PRs against `main` directly.
