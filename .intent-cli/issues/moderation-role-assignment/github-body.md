## Goal

Admin が他アカウントの InstanceRole(Admin/Moderator) を管理できる REST API を実装する。
PermissionWriter trait の具体実装を追加し、Moderator ロール保有者に suspend/unsuspend
操作を許可する権限チェックを接続する。ロールの永続化と一覧参照も含む。

## Why This Slice Exists Now

InstanceRole と PermissionChecker/PermissionWriter trait は
`kernel/src/permission.rs` に定義済みだが、PermissionWriter の具体実装がなく、
ロールを付与・剥奪する API も存在しない。Suspend/Ban API は実装済みだが
権限チェックが未接続のため、Moderator が実際にモデレーション操作を行えない状態である。

## Current Observed State

- `kernel/src/permission.rs`: InstanceRole(Admin, Moderator)、AccountRelation(Owner, Editor,
  Signer) が enum として定義されている。PermissionChecker trait(check/satisfies)、
  PermissionWriter trait(create_relation/delete_relation)、DependOnPermissionChecker、
  DependOnPermissionWriter が定義されている。いずれも具体実装は存在しない。
- `server/src/route/account/admin.rs`: suspend_account_by_id、unsuspend_account_by_id、
  ban_account_by_id の 3 エンドポイントが実装済み。JWT 認証済みで
  Extension<AuthClaims> 経由のリクエストを受け取る。
- ロール割当・一覧に関するテーブル・エンティティ・API は存在しない。
- PermissionWriter の具体実装は driver 層に存在しない。

## Accepted Baseline You May Assume

- `kernel/src/permission.rs` の trait 定義を変更せず、具体実装を driver 層に追加する
- CQRS CommandProcessor/QueryProcessor パターンに従う(既存の Account/AuthAccount の実装に倣う)
- ルーティング・DI は `server/src/route/account/admin.rs` と `AppModule` の既存構成に従う
- DB マイグレーションの追加方法は既存マイグレーションに従う

## Target Repo / Path / Part

Repository: `ShuttlePub/Emumet`

Target paths: `kernel/src/permission.rs`, `application/src/service`, `server/src/route`, `driver`

Target part: インスタンスロール(Admin/Moderator)の付与・剥奪・一覧 API、PermissionWriter trait の具体実装

## In Scope

- PermissionWriter trait の具体実装(権限バックエンド構成の調査と決定を含む)
- ロール付与・剥奪・一覧の REST API(Admin のみ実行可能)
- ロール変更の永続化方式の決定と実装
- Moderation ロールに基づく権限チェックの接続(Moderator → suspend/unsuspend)
- 上記のテスト

## Out Of Scope

- 通報(AccountReport) のユーザー向け受付・一覧・クローズ
- ホスト(リモートインスタンス)単位のモデレーション(HostModeration)
- AccountRelation(Owner/Editor/Signer) の変更 API
- 連合へのロール情報の配送

## Standalone Child Issue Contract

Emumet のモデレーション基盤の最初のスライスとして、InstanceRole の割当管理を追加する。
Admin が他アカウントに Admin/Moderator ロールを付与・剥奪し、一覧を参照できる REST API を
提供する。PermissionWriter trait の具体実装を既存の権限バックエンド構成に合わせて追加し、
Moderator ロール保有者に suspend/unsuspend 操作を許可する権限チェックを接続する。

## Acceptance Criteria

- [ ] Admin が他アカウントへ Moderator ロールを付与・剥奪できる
- [ ] Admin が他アカウントのロール一覧を参照できる
- [ ] Moderator ロール保持者が suspend/unsuspend を実行でき、非保持者は拒否される
- [ ] `cargo test` が通り、追加機能のテストが含まれる

## Verification

- `cargo test` (DATABASE_URL が必要なテストは既存方針に従う)
- Moderator ロールの付与→suspend 成功、剥奪→suspend 拒否の振る舞いテスト
- `git diff --check`

## Related Links

- intents/emumet/features/moderation/ (overview / requirements)
- https://github.com/ShuttlePub/Emumet/blob/main/kernel/src/permission.rs (権限モデル定義)
- https://github.com/ShuttlePub/Emumet/blob/main/server/src/route/account/admin.rs (既存 Admin API)

## Knowledge Maintenance

- Intent placement: intents/emumet/features/moderation(新規ノード不要)
- ADR candidate: none
- Diagram candidate: none
- Docs update: 実装状況確定後に ShuttlePub/document 側へ反映(別リポジトリ・別途)
- Closeout writeback expected: no(権限バックエンド選定は PR 說明と closeout で記録)

## Base Branch Policy

Policy: `direct-main`
Expected PR base branch: `main`

Open all child PRs against `main` directly.
