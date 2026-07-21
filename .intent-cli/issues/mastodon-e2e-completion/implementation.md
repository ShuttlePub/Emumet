# mastodon-e2e-completion Implementation Packet

## Goal

Iceshrimp 側で完成済みの ActivityPub 連携 E2E テスト (S7-S9) と同等のシナリオを、
Mastodon v4.6.2 インスタンスに対して実装する。フォロー双方向配送、Actor 解決、
署名付き Create/Note の inbox 配送まで、Mastodon とのクロスインスタンス連携を
 E2E で検証できるようにする。

## Why

GitHub issue #2 に「Mastodonとのe2eテスト実装」と記載されている。compose 定義
(Mastodon v4.6.2) とテストファイル・サポートコードの骨格は存在するが、テストケース
が未完成である。Iceshrimp 側は S7-S9 として完成済みであり、Mastodon 側も同等の
品質で E2E テストを揃えることが、ActivityPub 連携の正確な動作保証に不可欠である。

## Scope

- `e2e_ap_mastodon.rs` に Iceshrimp 版 S7-S9 と同等のテストケースを実装する
  - S7: Mastodon → Emumet フォロー (Actor 解決 → Follow 配送 → コレクション更新確認)
  - S8: Emumet → Mastodon フォロー (REST API → Follow 配送 → Mastodon 側フォロワー反映)
  - S9: Emumet → Mastodon 署名付き Create/Note 配送 (inbox POST → タイムライン反映確認)
- `run-ap-e2e.sh` への Mastodon テスト実行統合 (Mastodon コンテナ起動、ヘルスチェック、テスト実行)
- 必要に応じて nginx/証明書の SAN 対応確認 (mastodon.127.0.0.1.nip.io)
- Mastodon の OAuth2 認証フロー (アプリ登録 → クライアントトークン → アカウント作成 → 確認)

## Out of scope

- 新しい ActivityPub 機能の実装。テストは既存機能の検証に留める。機能不足が
  判明した場合は別 issue を作成して対応する
- CI への組み込み。ローカル実行 (`bash e2e/run-ap-e2e.sh`) で完結させる
- Mastodon との Block/Mute 連携テスト (block-mute-federation のスコープ)

## Verification

- `cargo test -p server --test e2e_ap_mastodon -- --ignored --test-threads=1 --nocapture` が通ること
- `bash e2e/run-ap-e2e.sh` で Mastodon テストを含む全 E2E が再現可能に実行できること
- `git diff --check`

## Knowledge Maintenance (G461, optional)

- Intent placement: intents/emumet/features/ap-federation(新規ノード不要、packets.md 追記のみ)
- ADR candidate: なし
- Diagram candidate: なし
- Docs update: テスト手順の記述を README または e2e ドキュメントに反映(既存 README の AP E2E 構成セクションを拡張)
- Closeout learning: Mastodon v4.6.x との連携で判明した相互運用上の注意点を closeout コメントで共有
