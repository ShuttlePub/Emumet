# media-upload — requirements

> See [overview.md](overview.md) for goals.

## Functional requirements

- 画像アップロード API(multipart 等)。MIME/type・サイズ上限のバリデーション
- Image エンティティ(id, url, hash, blur_hash)への登録。blurhash 生成
- 発行された URL の配信方法(直接配信 or プロキシ)
- Profile.icon / Profile.banner への紐付け
- Actor ドキュメントの icon/image 出力

## Non-functional

- ストレージバックエンドは差し替え可能な抽象化にする(open questions 参照)
