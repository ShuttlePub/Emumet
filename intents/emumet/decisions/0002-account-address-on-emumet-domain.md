# 0002: アカウントの住所は Emumet ドメインとし、投稿は ShuttlePub へ転送する

- Status: Accepted (2026-07-22 interview で確認)
- Deciders: operator

## Context

ShuttlePub 全体の分散思想では、タイムライン構築は本体サービス(ShuttlePub)が担う。
一方 ActivityPub の仕様上、投稿の配送先はアカウントの住所(inbox URL)に紐づく。
両立させる必要があった。

## Decision

- アカウントの住所(acct / Actor URL / inbox)は Emumet のドメインで提供する
- Emumet は受信した投稿系アクティビティを、アカウントごとの連携先 ShuttlePub
  サーバーへ転送する(features/post-relay, features/shuttlepub-link)
- ユーザーは利用する本体サービスを変えても住所・フォロワー関係・署名鍵を維持できる

## Consequences

- Emumet が連合との境界(プロキシ的役割)になる
- ShuttlePub 本体は ActivityPub の配送・署名を意識せずタイムライン構築に集中できる
- 転送プロトコルは未決定(post-relay の open question)

## Links

- [features/post-relay](../features/post-relay/overview.md)
- [interview 2026-07-22](../interview/2026-07-22-initial-shaping.md)
