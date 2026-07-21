# moderation — requirements

> See [overview.md](overview.md) for goals.

## Functional requirements

### ロール割当

- InstanceRole(Admin/Moderator)の付与・剥奪を管理する API
- PermissionWriter trait の具体実装(現状の権限バックエンド構成に合わせる)

### 通報 (AccountReport)

- 通報の作成(reporter, target, type, comment)・一覧・状態遷移(open/closed + close_reason)
- モデレーター向けの通報ハンドリング API

### ホストモデレーション

- リモートホスト単位の制限(suspend 等)と、その連合動作への反映方針
