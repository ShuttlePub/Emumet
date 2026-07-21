# post-relay — open questions

> See [../../clarifications/open.md](../../clarifications/open.md) for domain-level open questions.

## Open questions blocking this feature

- **ShuttlePub への転送プロトコル**: HTTP webhook / キュー(rikka-mq/Redis) / gRPC 等。
  ShuttlePub 本体側の受け口設計と合わせて決定が必要
- ShuttlePub からの投稿受け口の認証方式(サービス間認証)
- 転送に失敗した場合の再送・保管方針
- 外向き配送で既存の内部署名 API(`POST /internal/v1/accounts/{id}/sign`)を
  そのまま使うのか、Emumet 主導の配送に置き換えるのか
