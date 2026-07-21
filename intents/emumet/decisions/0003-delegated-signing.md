# 0003: ShuttlePub 発の投稿は Emumet が代理署名する

- Status: Accepted (2026-07-22 interview で確認)
- Deciders: operator

## Context

ActivityPub のサーバー間配送には HTTP Signature が必要で、署名鍵はアカウントに紐づく。
投稿データは ShuttlePub 本体が持つが、秘密鍵は Emumet が管理する。

## Decision

- 署名用秘密鍵は Emumet が生成・暗号化保管する(実装済み: `driver/src/crypto/rsa.rs`)
- ShuttlePub 発の投稿は Emumet が代理で署名し、外部へ配送する
- 現状の内部署名 API(`POST /internal/v1/accounts/{id}/sign`)はこの方針の一部実装。
  配送まで Emumet が担うかは post-relay の open question で確定させる

## Consequences

- 秘密鍵が ShuttlePub 本体側に出ない
- マスターキーパスワードによる鍵暗号化が運用要件になる(実装済み)

## Links

- [features/post-relay](../features/post-relay/overview.md)
- [0002](0002-account-address-on-emumet-domain.md)
