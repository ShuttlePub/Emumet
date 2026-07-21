# Open Clarifications

2026-07-22 stack 時点で packet 化を deferred した事項。`grill` / `clarification` で解消する。

## C1: media-upload のストレージバックエンド

- 背景: Image エンティティ/Repository は存在するが、アップロード API とストレージ連携がない
- 論点: S3 互換 / ローカル FS / ShuttlePub 側で保持、の何にするか。配信ドメインも未決
- 参照: ../features/media-upload/open-questions.md

## C2: post-relay の ShuttlePub 転送プロトコル

- 背景: inbox で受けた投稿を連携先 ShuttlePub へ転送する方式が未決
- 論点: HTTP webhook / キュー(rikka-mq/Redis) / その他。ShuttlePub 本体側の受け口設計と
  セットで決める必要がある。サービス間認証方式も未決
- 参照: ../features/post-relay/open-questions.md

## C3: shuttlepub-link の連携先認証・ multiplicity

- 背景: docs の StellarAccount 定義(access_token/refresh_token)を踏襲するか未決。
  ShuttlePub 本体側の実装状況の確認が必要
- 論点: 認証方式、1アカウントの連携先は 1 つか複数か、登録主体は誰か
- 参照: ../features/shuttlepub-link/open-questions.md
