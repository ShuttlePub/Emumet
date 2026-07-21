# post-relay — requirements

> See [overview.md](overview.md) for goals.

## Functional requirements

### 内向き(連合 → ShuttlePub)

- inbox で Create/Note を受け付け、HTTP Signature 検証後に連携先 ShuttlePub へ転送
- 転送先は shuttlepub-link で設定されたアカウントごとの連携先サービス
- 未設定アカウント宛ての挙動を定義(保留/破棄/エラー)

### 外向き(ShuttlePub → 連合)

- ShuttlePub から投稿データを受け取る内部 API
- Emumet が代理署名し、対象リモート inbox へ配送(OutboxActivity として記録)
- 配送失敗時のリトライ方針(既存 delivery 機構の拡張)

### 追加アクティビティ

- Like / Announce / Delete / Update 等のハンドリング方針を packet 単位で段階的に
