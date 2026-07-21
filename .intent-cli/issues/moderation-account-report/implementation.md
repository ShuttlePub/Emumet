# moderation-account-report Implementation Packet

## Goal

ユーザーが他アカウントを通報できる機能のドメイン基盤(エンティティ・EventStore・ReadModel・ユースケース)と
REST API を実装する。モデレーター向けの一覧・詳細・クローズ API も含み、権限チェック付きで提供する。
ActivityPub 連合(Flag アクティビティ)やホストモデレーションは別スライスで扱う。

## Why

docs(data-structure.md) に account_report_created / updated / closed のイベント定義が存在するが、
実際のエンティティ・テーブル・API は一切実装されていない。
2026-07-22 の interview で「アドミン達がモデレーションをちゃんとできる状態」に向けた
スコープとして決定された。通報受付・一覧・クローズはモデレーション運用の最低限必要機能。

## Scope

- AccountReport エンティティ(target, reported_by, type, comment, 状態 open/closed + close_reason)
- kernel interface traits: AccountReportEventStore / AccountReportReadModel
- Event Sourcing パターンに従い永続化(CQRS 構成)
  - event_store: account_report_events テーブル(作成・更新・クローズイベント)
  - read_model: account_reports テーブル(プロジェクション)
- マイグレーション(2 テーブル分)
- 通報作成 API: POST /api/v1/accounts/{id}/report (認証済みユーザー全員)
- 通報一覧 API: GET /api/v1/admin/reports (Moderator / Admin ロール保持者のみ)
- 通報詳細 API: GET /api/v1/admin/reports/{id} (Moderator / Admin ロール保持者のみ)
- 通報クローズ API: POST /api/v1/admin/reports/{id}/close (Moderator / Admin ロール保持者のみ, close_reason 付き)
- 各 API のテスト

## Out of scope

- 通報に対するモデレーションアクション実行の自動化(例: 通報数 threshold での自動suspend)
- リモートインスタンスへの通報連合(ActivityPub Flag アクティビティの配送/受信)
- ホスト(リモートインスタンス)単位のモデレーション(HostModeration)

## Verification

- `cargo test` で EventStore・ReadModel・ユースケースのテストが通ること
- 権限チェック: 非モデレーターが管理 API を叩いて 403 が返ること
- `git diff --check`

## Knowledge Maintenance (G461, optional)

- Intent placement: intents/emumet/features/moderation(新規ノード不要)
- ADR candidate: なし(Event Sourcing は既存 CQRS パターンに従い、PR 説明と closeout で判断理由を記録)
- Diagram candidate: なし
- Docs update: AccountReport イベント定義を実装に合わせて ShuttlePub/document 側へ反映(別リポジトリ・別途)
- Closeout learning: 永続化方式と close_reason スキーマの最終判断を closeout コメントで共有
