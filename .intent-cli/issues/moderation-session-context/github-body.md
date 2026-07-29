## Goal

認証済みセッションの admin/moderator ロールを Keto から取得し、
`GET /api/v1/me` で `{ "account_id": "...", "instance_roles": ["admin", ...] }` として返す。
フロントエンドはこのレスポンスを UI 表示制御に使い、認可境界は従来の Keto check に委ねる。

## Why This Slice Exists Now

2026-07-22 の intent interview で「管理者がモデレーションできる状態まで実装したい」と決定され、
ADR 0004 で「Keto をロールの唯一正源とし、BFF へは `GET /api/v1/me` で提供する」と承認された。
本 issue はロール割当 API とは独立して、セッション context の取得だけを提供する。

## Current Observed State

- `InstanceRole` (`Admin` / `Moderator`) と `PermissionChecker` trait は既に `kernel/src/permission.rs` に存在する
- `KetoClient` (`driver/src/keto.rs`) は `check` と `create/delete_relation` を実装済みだが、ロール一覧メソッドは未実装だった
- `GET /api/v1/me` エンドポイントは存在しなかった

## Accepted Baseline You May Assume

- Keto 上の `Instance` namespace / `singleton` object / `admins` または `moderators` relation がロールの正源
- JWT middleware 経由で `Extension<AuthClaims>` を受け取り、`resolve_auth_account_id` で `AuthAccountId` を解決する
- `AppModule` への DI 配線は `handler.rs` の既存パターンに従う
- テストは `test_with::env(DATABASE_URL)` で DB 依存テストをスキップする既存方式に従う

## Target Repo / Path / Part

Repository: `ShuttlePub/Emumet`

Target paths: `kernel/src/permission.rs`, `driver/src/keto.rs`, `application/src/service/session_context.rs`, `application/src/permission.rs`, `server/src/route/me.rs`, `server/src/openapi.rs`, `server/src/handler.rs`

Target part: 認証済みセッションの instance roles 取得と `GET /api/v1/me` 提供

## In Scope

- `PermissionChecker::list_instance_roles` の追加と `kernel` 単体テスト
- `KetoClient::list_instance_roles` の `GET /relation-tuples` 実装
- `GetSessionContextUseCase` の追加とテスト
- `GET /api/v1/me` ルート、レスポンススキーマ、OpenAPI 定義、server 結合テスト
- `AppModule` への KetoClient 配線

## Out Of Scope

- Admin/Moderator ロールの付与・剥奪 API (`moderation-role-assignment`)
- `PermissionWriter` の実装やその他の権限操作
- 他のエンドポイントでモデレーション操作を行う際の Keto check 本体

## Standalone Child Issue Contract

Emumet において、認証済みセッションの `account_id` と Keto 正源の `instance_roles` を
`GET /api/v1/me` から返す REST エンドポイントを追加する。
Keto が利用不能な場合は `503`、認証がない場合は `401` を返し、ロールがない場合のみ `200` かつ空配列を返す。

## Acceptance Criteria

- [ ] `GET /api/v1/me` (Bearer) が account_id と instance_roles 配列を 200 で返す
- [ ] Keto にロールがない場合は `200 OK + []`
- [ ] Keto 利用不能時は `503 Service Unavailable` を返し、空配列にフォールバックしない
- [ ] Authorization ヘッダなしは `401 Unauthorized`
- [ ] kernel 87 / driver 119 / application 31 / server 41 のテストがすべて pass する
- [ ] `cargo fmt` と `git diff --check` が pass する

## Verification

- `cargo test` (DATABASE_URL が必要なテストは既存方針に従う)
- `cargo fmt`
- `git diff --check`
- 実サービス手動 QA: 2026-07-28 実施。詳細は `.intent-cli/issues/moderation-session-context/review-context.md` の「Manual QA Evidence」セクションを参照。

## Related Links

- ADR: `intents/emumet/decisions/0004-admin-moderator-roles-frontend-keto-me.md`
- Feature: `intents/emumet/features/moderation/`
- Plan success criterion: `.omo/plans/ratcap-session-context-endpoint.md` の「実サービス curl 手動検証の実施・記録」

## Knowledge Maintenance

- Intent placement: `intents/emumet/features/moderation` (新規ノード不要。`packets.md` からの参照が実在する packet ディレクトリになる)
- ADR candidate: none (ADR 0004 済み)
- Diagram candidate: none
- Docs update: none (BFF 表示制御は別リポジトリ)
- Closeout writeback expected: no (実サービス QA 証跡は review-context.md に記録済み)

## Base Branch Policy

Policy: `direct-main`
Expected PR base branch: `main`

Open all child PRs against `main` directly.
