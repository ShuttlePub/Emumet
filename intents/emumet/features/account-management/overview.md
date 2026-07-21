---
facets: [invariant]
---

# account-management — overview

## Goals

ShuttlePub 全体で共有されるアカウントの正本を Event Sourcing + CQRS で管理する。
**基盤は実装済み**(2026-07 時点)。

## 実装済みスコープ

- Account CRUD + Deactivate(Event Sourced: Created/Updated/Deactivated/Suspended/Unsuspended/Banned)
- Profile(display_name, summary, icon, banner)・ Metadata(label, content) — Account 更新 API に統合
- AuthAccount / AuthHost による Ory(Kratos/Hydra)連携の認証解決
- 署名鍵管理(RSA 生成・暗号化保存・失効)と内部署名 API
- 楽観的排他(prev_version)・ Signal→Applier による ReadModel 投影

## Related

- [packets.md](packets.md)
- コード: `kernel/src/entity/account.rs`, `application/src/service/account*`, `server/src/route/account/`
- モデレーション関連は [../moderation/overview.md](../moderation/overview.md)
