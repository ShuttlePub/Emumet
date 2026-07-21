# block-mute — requirements

> See [overview.md](overview.md) for goals.

## Functional requirements

### エンティティ/データ

- Block 関係を表現するエンティティ。Follow と同様に local/remote を識別する
  source/destination 構造を踏襲する
- 永続化方式(純粋 CRUD Repository vs Event Sourcing)は packet 作成時に既存方針
  (Follow は ES を外した経緯あり)に合わせて決定
- Mute は Block より弱い関係。別テーブルか type カラムかは packet 時に決定

### REST API

- ブロック: 作成・一覧・解除
- ミュート: 作成・一覧・解除

### 連合

- ローカル → リモートへの Block は相手 inbox へ署名付き配送
- inbox で Block / Undo(Block) を受信処理
- ブロック成立時に既存の Follow/フォロワー関係を解除し、必要に応じて Reject/Undo を連合へ通知
