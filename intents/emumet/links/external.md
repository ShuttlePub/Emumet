# External Links

- ドキュメント: https://docs.shuttle.pub/docs/emumet
  (ソース: ShuttlePub/document リポジトリ `packages/document/docs/emumet/`)
- DB スキーマ図: ShuttlePub/document `packages/document/dbml/emumet.dbml` (dbdocs.io)
- Target repo: https://github.com/ShuttlePub/Emumet
- 関連リポジトリ ( ShuttlePub モノレポ群 ):
  - ShuttlePub/document — ドキュメント
  - ShuttlePub/Stellar — 認可サーバー構想(凍結)
  - ShuttlePub/Ratcap — 関連サービス
- 既存 TODO issue: https://github.com/ShuttlePub/Emumet/issues/2

## 注意: docs の乖離

docs.shuttle.pub の data-structure.md は実装より古い。Moderation 系イベントは
「未実装」と記載があるが、Suspend/Ban + Admin/Moderator ロールは実装済み。
StellarAccount イベント定義は「連携先 ShuttlePub サービス設定」として再解釈される
(features/shuttlepub-link 参照)。
