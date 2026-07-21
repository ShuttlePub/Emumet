---
facets: [invariant]
---

# moderation — overview

## Goals

サービスの admin / モデレーターが適切にモデレーションできる状態を完成させる
(2026-07-22 interview: 「ロールシステムの一部だけ作ったけど、まだ不足している。
サービスのアドミン達がモデレーションをちゃんとできるような状態まで実装したい」)。

## 現状(実装済み)

- Account ステータス: Active / Suspended(reason, expires_at 付き) / Banned
- Admin API: suspend / unsuspend / ban(`server/src/route/account/admin.rs`)
- 権限モデル: InstanceRole(Admin, Moderator)、AccountRelation(Owner, Editor, Signer)、
  PermissionChecker/PermissionWriter trait(`kernel/src/permission.rs`)

## 残スコープ

- ロール割当管理(Admin/Moderator の付与・剥奪 API。PermissionWriter の具体実装)
- 通報(AccountReport): ユーザーからの通報受付・一覧・クローズ
- ホスト(リモートインスタンス)単位のモデレーション(HostModeration 相当)
- モデレーション操作の監査的な記録方針(docs の *_moderation イベント群との対応整理)

## Related

- [requirements.md](requirements.md) / [packets.md](packets.md)
- docs: https://docs.shuttle.pub/docs/emumet/data-structure (Moderator/HostModeration/AccountReport イベント)
  ※ docs は「未実装」記載だが一部実装済みで docs が古い(links/external.md 参照)
