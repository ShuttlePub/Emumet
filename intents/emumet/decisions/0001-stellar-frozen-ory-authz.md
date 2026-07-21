# 0001: Stellar 凍結と Ory への認証認可委譲

- Status: Accepted (2026-07-22 interview で確認)
- Deciders: operator

## Context

Stellar は認可サーバーになる予定だったサービスだが、構築が凍結された。
認証・認可基盤が別途必要だった。

## Decision

- 認証は Ory Kratos、OAuth2/OIDC 認可は Ory Hydra に委譲する
- Stellar の残責務のうち「利用する ShuttlePub サービスの保存」は Emumet が引き継ぐ
  (features/shuttlepub-link として再定義)
- Emumet は Hydra の Login/Consent Provider として振る舞う

## Consequences

- Keycloak から Ory への移行が完了し、JWT `sub`(Kratos identity UUID)が
  AuthAccount.client_id に対応する
- StellarAccount イベント定義(docs)は ShuttlePub 連携設定として再解釈される

## Links

- [features/shuttlepub-link](../features/shuttlepub-link/overview.md)
- [interview 2026-07-22](../interview/2026-07-22-initial-shaping.md)
