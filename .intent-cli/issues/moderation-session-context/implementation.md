# moderation-session-context Implementation Packet

## Goal

認証済みセッションの admin/moderator ロールを Keto から取得し、
`GET /api/v1/me` で `{ "account_id": "...", "instance_roles": ["admin", ...] }` として返す。
フロントエンド(BFF)はこのレスポンスを UI 表示制御に使い、認可境界自体は従来の Keto check に委ねる。

## Why

2026-07-22 の interview で「管理者がモデレーションできる状態まで実装したい」と決定され、
ADR 0004 で「Keto をロールの唯一正源とし、BFF へは `GET /api/v1/me` で提供する」と承認された。
本 slice はロール割当 API (`moderation-role-assignment`) とは独立して動作し、
セッション context だけを提供する。

## Scope

- `kernel/src/permission.rs`: `PermissionChecker::list_instance_roles` と `InstanceRole` の定義
- `driver/src/keto.rs`: `KetoClient` の `GET /relation-tuples` によるロール一覧実装
- `application/src/service/session_context.rs`: `GetSessionContextUseCase`
- `server/src/route/me.rs`: `GET /api/v1/me` ハンドラ、JWT 認証、OpenAPI 注釈
- `server/src/handler.rs`: `AppModule` への `PermissionChecker` / KetoClient 配線
- 上記のテスト (kernel 単体テスト、server 結合テストを含む)

## Out of scope

- Admin/Moderator ロールの付与・剥奪 API (`moderation-role-assignment`)
- `PermissionWriter` の実装やその他の権限操作
- 他のエンドポイントでモデレーション操作を行う際の Keto check 本体

## Verification

- 自動テスト: `cargo test` (DATABASE_URL 環境変数が必要なテストは既存方針に従う)
- テスト結果:
  - kernel 87 pass
  - driver 119 pass
  - application 31 pass
  - server 41 pass
- `cargo fmt` パス
- `git diff --check` パス
- 実サービス手動 QA: 2026-07-28 実施。詳細は `review-context.md` の「Manual QA Evidence」セクションに記録。

## Knowledge Maintenance (G461, optional)

- Intent placement: `intents/emumet/features/moderation` (新規ノード不要、既存 packets.md への参照を実在させる)
- ADR candidate: なし (ADR 0004 がすでに承認済み)
- Diagram candidate: なし
- Docs update: なし (BFF 側の表示制御は別リポジトリ)
- Closeout learning: 実サービス QA 証跡を review-context.md に記録済み。追加 writeback はなし。

`improve` (G456 / G460) is the later safety net; packet-time maintenance is the normal path.
