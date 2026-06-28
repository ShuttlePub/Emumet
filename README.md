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

ActivityPub 連携の E2E テストは Iceshrimp/Mastodon インスタンスを含む Docker Compose 環境で実行します。

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

Iceshrimp (`iceshrimp.dev/iceshrimp/iceshrimp:v2026.5.1`) と Mastodon (`ghcr.io/mastodon/mastodon:latest`) を含む全サービスが起動します。

nginx リバースプロキシが HTTPS (port 8443) で全ドメインを統一的にルーティング:

| Domain | Backend | 説明 |
|--------|---------|------|
| `emumet.127.0.0.1.nip.io:8443` | host.docker.internal:8080 | Emumet (ホスト実行) |
| `peer.127.0.0.1.nip.io:8443` | host.docker.internal:18081 | Mock AP Peer (テスト内実行) |
| `iceshrimp.127.0.0.1.nip.io:8443` | iceshrimp:3000 | Iceshrimp (Docker) |
| `mastodon.127.0.0.1.nip.io:8443` | mastodon-web:3000 | Mastodon (Docker) |

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
export PUBLIC_BASE_URL="https://emumet.127.0.0.1.nip.io:8443"
export ICESHRIMP_BASE_URL="https://iceshrimp.127.0.0.1.nip.io:8443"
cargo run -p server --features test-mode &

# 6. テスト実行
cargo test -p server --test e2e_ap_mock -- --ignored --test-threads=1 --nocapture
cargo test -p server --test e2e_ap_iceshrimp -- --ignored --test-threads=1 --nocapture

# 7. 後片付け
kill %1 2>/dev/null; wait 2>/dev/null
docker compose --profile ap-e2e -f compose.yml -f compose.ap-e2e.yml down
```

### テスト一覧

| ID | テスト | 説明 |
|----|--------|------|
| S1 | `webfinger_resolves_account` | WebFinger アカウント解決 |
| S2 | `actor_document_is_valid_activitypub` | Actor 文書 + publicKey |
| S3 | `outbound_follow_sends_activity_to_remote_inbox` | Mock peer への Follow 配送 |
| S4 | `inbound_follow_creates_follower_and_sends_accept` | Cavage 署名 Follow → followers 反映 |
| S5 | `followers_and_following_collections_are_accurate` | コレクション整合性 |
| S6 | `inbox_rejects_unsigned_requests` | 未署名リクエスト拒否 |
| S7 | `iceshrimp_follows_emumet_account` | Iceshrimp → Emumet クロスインスタンス Follow |

### ファイル構成

```
server/tests/
├── e2e_ap_mock.rs          # S1-S6 Mock peer テスト
├── e2e_ap_iceshrimp.rs     # S7 Iceshrimp 連携テスト
└── support/
    ├── ap_peer.rs           # Mock AP peer + HTTP Signature
    ├── iceshrimp.rs         # Iceshrimp REST API クライアント
    ├── account_helper.rs    # 共通テストヘルパー
    ├── config.rs            # E2E 設定 (env var)
    └── server.rs            # EmumetServer (ext-server mode)
e2e/
├── run-ap-e2e.sh            # 自動ランナー
├── check-mastodon.sh        # Mastodon 健全性確認
├── certs/
│   ├── gen-cert.sh          # 証明書生成 (4 SAN ドメイン)
│   └── nginx.conf           # nginx リバースプロキシ設定
└── iceshrimp/
    └── .config/
        ├── default.yml      # Iceshrimp 設定
        └── docker.env       # Iceshrimp DB 認証
```

## Etymology

EMU(Extravehicular Mobility Unit) + Helmet
