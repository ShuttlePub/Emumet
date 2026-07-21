# moderation-role-assignment Implementation Packet

## Goal

Admin が他アカウントの InstanceRole(Admin/Moderator) を付与・剥奪・参照できる REST API を実装する。
PermissionWriter trait の具体実装を既存の権限バックエンド構成に合わせて追加する。
Moderator ロール保持者に suspend/unsuspend を許可する権限チェックを接続する。

## Why

InstanceRole(Admin, Moderator) と PermissionChecker/PermissionWriter trait は
`kernel/src/permission.rs` に定義済みだが、PermissionWriter の具体実装が存在しない。
Suspend/Ban API は `server/src/route/account/admin.rs` に実装済みだが、
ロールの付与・剥奪を管理する API がなく、Moderator が実際に操作を行えない。

## Scope

- PermissionWriter trait の具体実装(現行の権限バックエンド構成に合わせる。外部依存が
  総む場合は構成を調査して既存方針に従う)
- ロール付与 API: Admin が他アカウントに Admin/Moderator ロールを付与できる
- ロール剥奪 API: Admin が他アカウントから Admin/Moderator ロールを剥奪できる
- ロール一覧 API: 指定アカウントの保有ロールを参照できる
- ロール変更の永続化方式の決定と実装
- Moderation ロールに基づく権限チェックの接続(Moderator → suspend/unsuspend 許可)
- テスト

## Out of scope

- 通報(AccountReport) ユーザーからの通報受付・一覧・クローズ
- ホスト(リモートインスタンス)単位のモデレーション(HostModeration)
- AccountRelation(Owner/Editor/Signer) の変更 API
- 連合へのロール情報の配送

## Verification

- `cargo test` でロール割当・権限チェックのテストが通ること
- Moderator ロール保持者の suspend 成功と非保持者の拒否の振る舞いテスト
- `git diff --check`

## Knowledge Maintenance (G461, optional)

- Intent placement: intents/emumet/features/moderation(新規ノード不要)
- ADR candidate: なし(権限バックエンドの選定は実装調査で決定、PR 説明と closeout で記録)
- Diagram candidate: なし
- Docs update: 実装状況確定後に ShuttlePub/document 側の data-structure.md へ反映(別途)
- Closeout learning: PermissionWriter の権限バックエンド選定の最終判断と理由を closeout コメントで共有
