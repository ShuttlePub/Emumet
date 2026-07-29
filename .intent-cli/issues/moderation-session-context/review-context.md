# moderation-session-context Review Context

Review that this slice moves operation toward the documented intent without widening scope.

Flag findings if the implementation:

- widens scope beyond the issue contract;
- launches AI providers from `intent-cli`;
- mutates GitHub or parent state when the issue is read-only;
- skips required contract sections.

## Facet context

<!-- BEGIN GENERATED FACET CONTEXT (G530) -->
### vocabulary
- (none overlapping this packet's intent_references)
### invariant
- (none overlapping this packet's intent_references)
### decider
- intents/emumet/decisions/0004-admin-moderator-roles-frontend-keto-me.md
### acceptance-property
- intents/emumet/features/moderation/acceptance.md
<!-- END GENERATED FACET CONTEXT (G530) -->

## Knowledge Writeback Expectation (G461)

If the packet's `closeout_learning.write_back_required` is `true`, confirm the
expected intent-tree / ADR / diagram / docs writeback landed in this PR or was
captured as a follow-up packet. If the packet declined all knowledge maintenance,
that is acceptable — note it rather than blocking.

This packet declined all knowledge maintenance except the intent-tree reference,
which is already recorded in `features/moderation/packets.md` and now has a real
packet directory.

## Manual QA Evidence

### 目的

ADR 0004 に基づき、`GET /api/v1/me` が Keto を正源として instance_roles を返し、
Keto 障害時は `503 Service Unavailable` を返すこと（`200 + 空配列` へのフォールバックはしない）、
認証なしの場合は `401 Unauthorized` となることを、実サービス curl で確認する。

### 環境

- 実施日: 2026-07-28
- ホスト: `cargo run -p server` による Emumet server on port `8080`
- バックエンド: `docker compose up -d` によるスタック。すべて healthy
  - PostgreSQL, Redis, Ory Kratos, Ory Hydra, Ory Keto (`4466` read / `4467` write)
- Hydra Admin API で `client_credentials` クライアント `qa-me-client` を作成
  - `audience=account`
  - JWT: `iss=http://localhost:4444/`, `aud=["account"]`
- `resolve_auth_account_id` の find-or-create により AuthAccount が自動作成
  - `account_id=75613492660404224`

### 実行コマンドと結果

S2: 認証あり、ロールなし

```bash
curl -s -H "Authorization: Bearer $TOKEN" http://localhost:8080/api/v1/me
```

結果:

```json
{"account_id":"75613492660404224","instance_roles":[]}
```

HTTP ステータス: `200 OK`

S1: Keto へ admin tuple を付与後、admin ロールを取得

```bash
curl -s -X PUT http://localhost:4467/admin/relation-tuples \
  -H "Content-Type: application/json" \
  -d '{"namespace":"Instance","object":"singleton","relation":"admins","subject_id":"75613492660404224"}'
```

結果: `201 Created`

その後:

```bash
curl -s -H "Authorization: Bearer $TOKEN" http://localhost:8080/api/v1/me
```

結果:

```json
{"account_id":"75613492660404224","instance_roles":["admin"]}
```

HTTP ステータス: `200 OK`

S3: Keto 障害時の挙動

```bash
docker stop emumet-keto
curl -s -H "Authorization: Bearer $TOKEN" http://localhost:8080/api/v1/me
```

結果: `503 Service Unavailable`

`200 + []` へのフォールバックは発生しないことを確認。

その後:

```bash
docker start emumet-keto
```

Keto が healthy になると、`GET /api/v1/me` から `admin` ロールが再び取得できたことを確認。

S5: Authorization ヘッダなし

```bash
curl -s http://localhost:8080/api/v1/me
```

結果: `401 Unauthorized`

### クリーンアップ

- Keto tuple 削除:

```bash
curl -s -X DELETE "http://localhost:4467/admin/relation-tuples?namespace=Instance&object=singleton&relation=admins&subject_id=75613492660404224"
```

結果: `204 No Content`。read API で tuple が空になったことを確認。

- Hydra クライアント削除:

```bash
curl -s -X DELETE "http://localhost:4445/admin/clients/qa-me-client"
```

結果: `204 No Content`

- Emumet server プロセスを停止。port 8080 が connection refused になることを確認。
- `docker compose` スタックは稼働のまま残した。

### 残留事項

- DB に `auth_hosts` (`iss=http://localhost:4444/`) と `auth_accounts` (`id=75613492660404224`) の find-or-create レコードが残存。
- これは認証フロー動作確認の副産物であり、問題ない。

### 留意点

- `application/session_context` テストの Snowflake ID 生成器初期化は、各テストの冒頭で `ensure_generator_initialized()` を呼ぶ形式に改善済み。これは別タスクで実施された。
