# Emumet

<a href="https://codecov.io/gh/ShuttlePub/Emumet" >
 <img src="https://codecov.io/gh/ShuttlePub/Emumet/branch/main/graph/badge.svg?token=NY4FA3YZPS"/>
 </a>

## Setup

### Services

```shell
podman-compose up -d
```

PostgreSQL, Redis, Ory Kratos, Ory Hydra が起動します。

### Auth: Ory Kratos + Hydra

- **Kratos** (Identity Management): http://localhost:4433
  - Test user: testuser@example.com / testuser
- **Hydra** (OAuth2/OIDC): http://localhost:4444

### Environment

```shell
cp .env.example .env
```

## DB

`podman-compose` で PostgreSQL が起動します。手動起動する場合:

```shell
podman run --rm --name emumet-postgres -e POSTGRES_PASSWORD=develop -p 5432:5432 docker.io/postgres
```

> User: postgres / Password: develop

## ActivityPub Federation E2E Tests

ActivityPub 連携の E2E テストは Iceshrimp インスタンスを含む Docker Compose 環境で実行します。

### Prerequisites

- Docker Compose v2 (または Podman Compose)
- Rust ツールチェーン (cargo)
- openssl (証明書生成用)
- curl (ヘルスチェック用)

### インフラ構成 (`compose.ap-e2e.yml`)

```yaml
# オーバーライドとして使用:
docker compose --profile ap-e2e -f compose.yml -f compose.ap-e2e.yml up -d
```

Iceshrimp (`iceshrimp.dev/iceshrimp/iceshrimp:v2026.5.1`) を含む全サービスが起動します。

nginx リバースプロキシが HTTPS (port 8443) で全ドメインを統一的にルーティング:

| Domain | Backend | 説明 |
|--------|---------|------|
| `emumet.127.0.0.1.nip.io:8443` | host.docker.internal:8080 | Emumet (ホスト実行) |
| `peer.127.0.0.1.nip.io:8443` | host.docker.internal:18081 | Mock AP Peer (テスト内実行) |
| `iceshrimp.127.0.0.1.nip.io:8443` | iceshrimp:3000 | Iceshrimp (Docker) |

### 自動ランナー

```shell
# 全自動: cert生成 → compose起動 → サーバー起動 → テスト実行
bash e2e/run-ap-e2e.sh
```

### 手動実行

```shell
# 1. 証明書生成
bash e2e/certs/gen-cert.sh

# 2. マスターキーパスワード
echo "develop" > master-key-password && chmod 600 master-key-password

# 3. ビルド
cargo build -p server --features test-mode

# 4. インフラ起動
docker compose --profile ap-e2e -f compose.yml -f compose.ap-e2e.yml up -d

# 5. サーバー起動
export AP_TEST_ALLOWED_FETCH_HOSTS="127.0.0.1,iceshrimp.127.0.0.1.nip.io"
export AP_TEST_ACCEPT_INVALID_CERTS=1
export EMUMET_TEST_MODE_TOKEN=<your-token>
export EMUMET_E2E_EXTERNAL_SERVER=1
export EMUMET_E2E_SERVER_BASE_URL="http://localhost:8080"
export EMUMET_E2E_PUBLIC_BASE_URL="https://emumet.127.0.0.1.nip.io"
export PUBLIC_BASE_URL="https://emumet.127.0.0.1.nip.io"
export ICESHRIMP_BASE_URL="https://iceshrimp.127.0.0.1.nip.io:8443"
cargo run -p server --features test-mode &

# 6. テスト実行
cargo test -p server --test e2e_basic_flow -- --ignored --test-threads=1 --nocapture
cargo test -p server --test e2e_ap_mock -- --ignored --test-threads=1 --nocapture
cargo test -p server --test e2e_ap_iceshrimp -- --ignored --test-threads=1 --nocapture

# 7. 後片付け
kill %1 2>/dev/null; wait 2>/dev/null
docker compose --profile ap-e2e -f compose.yml -f compose.ap-e2e.yml down
```

### Pause Before Cleanup Mode

テスト実行後、クリーンアップ（サーバー停止・コンテナ破棄）の直前に一時停止し、ブラウザから Iceshrimp の状態を確認できるモードです。

```shell
EMUMET_E2E_PAUSE_BEFORE_CLEANUP=1 bash e2e/run-ap-e2e.sh
```

停止中にアクセス可能なURL:

| URL | 対象 |
|-----|------|
| `https://iceshrimp.127.0.0.1.nip.io:8443` | Iceshrimp |
| `https://emumet.127.0.0.1.nip.io:8443` | Emumet |

Iceshrimp のテストユーザー認証情報は `run-ap-e2e.sh` の出力から確認できます（ユーザー名は `e2e_<timestamp>` 形式、パスワードは `test-pass`）。確認が終わったら Enter キーを押すとクリーンアップが続行されます。

### テスト一覧

連合は Profile 単位 (1 Profile = 1 actor, `{PUBLIC_BASE_URL}/ap/profiles/{nanoid}`) です。切替手順は [docs/profile-federation-cutover.md](docs/profile-federation-cutover.md)。

| テスト | 説明 |
|--------|------|
| `webfinger_resolves_the_profile_actor_by_username` | username 基準の WebFinger (Account 名は 404) |
| `actor_document_publishes_the_profile_key` | Profile actor 文書・公開鍵 (DB の鍵と一致) |
| `retired_account_federation_surface_answers_gone` | `/ap/accounts/...`・署名 API・Account 単位 Follow 等は 410 |
| `outbound_follow_delivers_a_follow_signed_with_the_profile_key` | Mock peer への Follow 配送、actor 文書の公開鍵で署名検証 |
| `follow_by_acct_resolves_the_peer_over_webfinger` | `acct:` 指定の Follow (peer の WebFinger を実 HTTP で解決) |
| `unfollow_delivers_undo_and_a_failing_peer_does_not_undo_the_unfollow` | Undo(Follow) 配送、peer 失敗でも unfollow は戻らず再試行待ち |
| `inbound_follow_creates_a_follower_and_delivers_a_verifiable_accept` | Cavage 署名 Follow (実 HTTP で peer 鍵を取得) → followers・Accept 配送・重複・Undo |
| `inbox_rejects_unsigned_requests` | 未署名リクエスト拒否 |
| `inbox_refuses_to_fetch_keys_from_internal_hosts_directly_or_by_redirect` | 鍵取得の SSRF 対策 (内部 host への直接指定・redirect) |
| `individual_block_detaches_relations_without_sending_an_activitypub_block` | 個別 block: 双方向解除 (Undo/Reject)、AP Block は送らない、解除しても復元しない |
| `inbound_activitypub_block_is_acknowledged_and_changes_nothing` | 受信 Block は 202 で無変更 |
| `mute_is_idempotent_per_target_and_changes_no_relation` | mute (409 重複、配送なし) |
| `usernames_with_dot_underscore_and_hyphen_are_actors` | `.` `_` `-` を含む username |
| `iceshrimp_full_federation_scenario` (S7-S10) | Iceshrimp → Emumet Follow → Emumet → Iceshrimp Follow → Profile 鍵で署名した Create/Note 配送 → 個別 block (Undo/Reject、AP Block なし) |
| `mastodon_full_federation_scenario` (S7-S10) | Mastodon → Emumet Follow → Emumet → Mastodon Follow → Profile 鍵で署名した Create/Note 配送 → 双方向 Undo(Follow) |

Create/Note の署名はサーバーに署名 API が無いため、テスト fixture (`support/profile_signer.rs`) が DB の Profile 鍵を master key password で復号して行います (配送 worker と同じ手順)。

### ファイル構成

```
server/tests/
├── e2e_ap_mock.rs          # Mock peer テスト (Profile 連合)
├── e2e_ap_iceshrimp.rs     # S7-S10 Iceshrimp 連携テスト
├── e2e_ap_mastodon.rs      # S7-S10 Mastodon 連携テスト
└── support/
    ├── ap_peer.rs           # Mock AP peer + HTTP Signature
    ├── iceshrimp.rs         # Iceshrimp REST API クライアント
    ├── account_helper.rs    # 共通テストヘルパー
    ├── config.rs            # E2E 設定 (env var)
    └── server.rs            # EmumetServer (ext-server mode)
e2e/
├── run-ap-e2e.sh            # 自動ランナー (AP E2E)
├── certs/
│   ├── gen-cert.sh          # 証明書生成 (3 SAN ドメイン)
│   └── nginx.conf           # nginx リバースプロキシ設定
└── iceshrimp/
    └── .config/
        ├── default.yml      # Iceshrimp 設定
        └── docker.env       # Iceshrimp DB 認証
```

## Etymology

EMU(Extravehicular Mobility Unit) + Helmet
