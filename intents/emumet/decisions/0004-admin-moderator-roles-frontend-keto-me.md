# 0004: admin/moderator ロールのフロントエンド伝達は Keto を正源とし GET /api/v1/me で提供する

- Status: Accepted (2026-07-28)
- Deciders: operator

## Context

- Emumet には `InstanceRole::Admin / Moderator` (`kernel/src/permission.rs`) と Keto ベースの `PermissionChecker` (`driver/src/keto.rs`) が既に存在する
- 運用ロールの正源は、既存 ADR [0001](0001-stellar-frozen-ory-authz.md) に従い Ory Keto である
- フロントエンド(BFF=RatCap)は、モデレーション UI や操作の表示制御のために、現在のセッションが admin/moderator かを知る必要がある
- この情報をどこから、どのタイミングで、どのような鮮度で提供するかを決める必要があった

## Decision

- Keto を admin/moderator ロールの唯一の正源と維持する
- 認証済みセッションのロールを `GET /api/v1/me` で返す
  - レスポンス例: `{ "account_id": "<AuthAccountId 文字列>", "instance_roles": ["admin", "moderator", ...] }`
  - ロールがない場合は `200 OK + 空配列`
  - Keto 障害時は `503 Service Unavailable`
- BFF(RatCap) はレスポンスを TTL キャッシュしてよい
- 実際の認可判定は、従来通り use case 内で Keto check を行う
  - `GET /api/v1/me` から得たロールは UI 表示制御専用とし、権限境界には使わない

## 鮮度方針

- ロール付与の即時反映はフロントエンドに必要ない
- ロール剥奪後の初回モデレーション操作時に `403 Forbidden` を受け取れば、フロントエンドはその時点で UI 状態を更新できる
- そのため TTL キャッシュは許容する

## 却下案と理由

- **Kratos `metadata_public` へのロール保存**
  - 理由: Keto との正源が二重化し、Keto 変更を Kratos metadata_public に同期する機構が必要になる
- **consent 時に JWT claim へロールを注入**
  - 理由: トークン寿命の間はロール変更が反映されない (staleness) ことと、consent フローが複雑になる

## Consequences

- BFF は `GET /api/v1/me` からロールを取得し、UI 表示を制御する
- Keto だけがロールの正源を維持され、同期機構が不要になる
- モデレーション操作の認可自体は Keto check に依存し続けるため、セキュリティ境界は変わらない

## Links

- [features/moderation/packets.md](../features/moderation/packets.md)
- [0001: Stellar 凍結と Ory への認証認可委譲](0001-stellar-frozen-ory-authz.md)
- `kernel/src/permission.rs` — `InstanceRole::Admin / Moderator`
- `driver/src/keto.rs` — Keto クライアント
