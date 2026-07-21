## Goal

Iceshrimp 側で完成済みの E2E テスト S7-S9 と同等の ActivityPub 連携シナリオを、
Mastodon v4.6.2 との間で E2E テストとして実装する。フォロー双方向配送、Actor 解決、
署名付き Create/Note の inbox 配送までをカバーする。

## Why This Slice Exists Now

GitHub issue #2 の未完了項目「Mastodonとのe2eテスト実装」であり、Iceshrimp 側
S7-S9 の完成を受け、 Mastodon でも同等の E2E 検証を揃えるため。compose 定義
(Mastodon v4.6.2) とテストファイル・サポートコードの骨格は存在するが、テスト
ケースが未完成である。

## Current Observed State

- `compose.ap-e2e.yml` に Mastodon v4.6.2 のサービス定義 (mastodon-db, mastodon-setup,
  mastodon-web, mastodon-sidekiq) が存在する
- nginx の SAN に `mastodon.127.0.0.1.nip.io` が含まれ、証明書生成スクリプト
  (`e2e/certs/gen-cert.sh`) でもこのドメインがカバーされている
- `server/tests/support/mastodon.rs` に Mastodon REST API クライアントの骨格が存在する
- `server/tests/support/mastodon_setup.rs` にアカウント作成・セットアップの骨格が存在する
- `server/tests/e2e_ap_mastodon.rs` にテストファイルの骨格が存在するが、テスト
  ケースの実装が未完了である
- `e2e/run-ap-e2e.sh` に Mastodon サービスのヘルスチェックとテスト実行ステップの
  骨格が存在する
- Iceshrimp 側の S7-S9 (`e2e_ap_iceshrimp.rs`) は完成済みであり、パターンの参照元
  となる

## Accepted Baseline You May Assume

- Iceshrimp 側 S7-S9 の実装パターン (`e2e_ap_iceshrimp.rs`) をそのまま Mastodon
  向けにアダプトできる
- Mastodon の OAuth2 API フロー (アプリ登録 → クライアントトークン → アカウント作成)
  は `mastodon.rs` クライアントで抽象化されている
- nginx の HTTPS リバースプロキシ (port 8443) と SAN 証明書は既に Mastodon ドメイン
  をカバーしている
- Emumet の test-mode API (`/__test__/health`, `/__test__/cache-actor-key`,
  `/__test__/cache-remote-actor`) は Iceshrimp テストと同じ方法で使える

## Target Repo / Path / Part

Repository: `ShuttlePub/Emumet`

Target paths: `server/tests`, `e2e`, `compose.ap-e2e.yml`

Target part: Mastodon との ActivityPub 連携 E2E テスト (S7-S9 相当)

## In Scope

- `e2e_ap_mastodon.rs` に Iceshrimp 版 S7-S9 と同等のテストケースを実装する
  - S7: Mastodon → Emumet フォロー (Actor 解決 → Follow 配送 → コレクション更新確認)
  - S8: Emumet → Mastodon フォロー (REST API → Follow 配送 → Mastodon 側フォロワー反映)
  - S9: Emumet → Mastodon 署名付き Create/Note 配送 (inbox POST → タイムライン反映確認)
- `run-ap-e2e.sh` への Mastodon テスト実行統合
- Mastodon の OAuth2 認証フローのサポートコード完成
- 必要に応じて nginx/証明書の SAN 対応確認

## Out Of Scope

- 新しい ActivityPub 機能の実装 (テストは既存機能の検証に留める。機能不足が判明
  した場合は別 issue を作成して対応する)
- CI への組み込み (ローカル実行で完結させる)
- Mastodon との Block/Mute 連携テスト (block-mute-federation のスコープ)

## Standalone Child Issue Contract

Mastodon v4.6.2 インスタンスとの ActivityPub 連携 E2E テストを完成させる。
Iceshrimp 側 S7-S9 と同等のシナリオ (フォロー双方向、Actor 解決、署名付き
Create/Note 配送) を Mastodon 向けに実装し、`bash e2e/run-ap-e2e.sh` で
再現可能に実行できるようにする。テストは既存機能の検証に留め、新機能の追加は
行わない。

## Acceptance Criteria

- [ ] Mastodon とのフォロー双方向シナリオ (S7-S8) が E2E で通る
- [ ] Emumet → Mastodon 署名付き Create/Note 配送 (S9) が E2E で通る
- [ ] `run-ap-e2e.sh` で Mastodon テストが再現可能に実行できる
- [ ] テスト手順が README または e2e ドキュメントに記載される
- [ ] `cargo test` の通常テストに影響を与えない (#[ignore] 属性で E2E のみ)

## Verification

- `cargo test -p server --test e2e_ap_mastodon -- --ignored --test-threads=1 --nocapture`
- `bash e2e/run-ap-e2e.sh` (全 E2E テスト含む実行)
- `git diff --check`

## Related Links

- https://github.com/ShuttlePub/Emumet/issues/2 (元 TODO)
- intents/emumet/features/ap-federation/ (overview)

## Knowledge Maintenance

- Intent placement: intents/emumet/features/ap-federation (packets.md 追記のみ)
- ADR candidate: none
- Diagram candidate: none
- Docs update: テスト手順を README の AP E2E 構成セクションに反映
- Closeout writeback expected: no

## Base Branch Policy

Policy: `direct-main`
Expected PR base branch: `main`

Open all child PRs against `main` directly.
