# media-upload — open questions

> See [../../clarifications/open.md](../../clarifications/open.md) for domain-level open questions.

## Open questions blocking this feature

- **ストレージバックエンド選定**: S3 互換 / ローカル FS / ShuttlePub 側で持つか。
  開発環境(podman-compose)での構成も含めて決定が必要
- 画像の配信ドメイン(Emumet ドメインで配信するか、CDN/別ドメインか)
- アイコン/バナー変更時に Update アクティビティをフォロワーへ配送するか
