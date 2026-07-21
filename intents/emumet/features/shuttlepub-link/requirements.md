# shuttlepub-link — requirements

> See [overview.md](overview.md) for goals.

## Functional requirements

- アカウントごとに連携先 ShuttlePub サービス(host 等)を登録・更新・削除できる
- 連携先との認証に必要なクレデンシャル管理(docs の access_token/refresh_token 相当。
  ただし実際の方式は ShuttlePub 本体側の設計次第)
- post-relay が転送先を解決するための参照インターフェース
- 永続化方式: 既存の CQRS/ES 方針に従う(Event Sourced entity 候補)
