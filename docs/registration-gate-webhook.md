# 登録ゲート通知の外部 Webhook (D15 / E15w)

Emumet は、登録ゲートの通知イベント (障害・復旧・防御低下・停止・override の開始/解除) を、管理画面を開いていなくても届くように
**サーバー設定の単一 HTTPS endpoint** へ署名付きで配送できる。この文書は **受信側** (Webhook を受け取るシステム) と運用者向けの契約。

## 1. 設定 (サーバー側のみ)

endpoint と署名 secret は **サーバー設定だけ**で管理する。管理 API からは書き換えられない (状態の表示・テスト送信・再試行のみ)。

| 環境変数 | 内容 |
|---|---|
| `REGISTRATION_WEBHOOK_URL` | 配送先 URL。`https://` のみ。userinfo (`user:pass@`) 不可。 |
| `REGISTRATION_WEBHOOK_SECRET` | 署名 secret (16 バイト以上。32 バイト以上のランダム値を推奨)。 |
| `REGISTRATION_WEBHOOK_SECRET_FILE` | secret をファイルから読む場合のパス。`SECRET` と同時指定は設定不備。 |
| `REGISTRATION_WEBHOOK_POLL_INTERVAL_MS` | 配送 worker の poll 間隔 (既定 2000)。 |

- **未設定 (URL も secret もない) は Webhook 無効**で、エラーではない。配送 worker は起動せず、状態 API は `not_configured` を返し、ゲートの動作には影響しない。
- 設定不備 (http、不正な URL、内部アドレス/localhost、secret 欠落・短すぎる等) は **起動失敗にならない**。状態 API が `misconfigured` と
  不備の分類 (`issue`) を返し、worker は動かず、ゲートの動作には影響しない。
- secret・URL の値は DB・API 応答・ログ・エラー・payload のどこにも出ない。状態 API が示す宛先は host 部分のみ。
- secret を変更するときは、サーバー設定を更新して再起動する。初版は単一 secret で、ローテーション用の二重受理はない。
  切替の間に旧 secret で署名された配送を受信側が新 secret で検証すると署名不一致になり、受信側が 401/403 等の 4xx を返すと
  **恒久失敗 (`failed`) になり自動では再送されない**。受信側を先に新 secret 対応にするか、切替後に状態 API で `failed` を確認して
  管理者が再試行 (§7) する運用にすること。

## 2. リクエスト

`POST <endpoint>`、`Content-Type: application/json`。リダイレクトは **追従しない** (3xx は恒久失敗)。

### ヘッダー

| ヘッダー | 内容 |
|---|---|
| `X-Emumet-Event-Id` | **安定した event ID**。再試行・crash 後の再送・管理者の再試行でも同一。重複排除のキー。 |
| `X-Emumet-Event-Type` | イベント種別 (下表)。 |
| `X-Emumet-Delivery-Attempt` | その event の通算試行番号 (1 始まり。管理者の再試行をまたいで増える)。 |
| `X-Emumet-Timestamp` | 送信時刻 (Unix 秒)。**試行ごとに更新される**。 |
| `X-Emumet-Signature` | `v1=<小文字 hex>` の HMAC-SHA256 署名 (§3)。 |

### イベント種別

| `type` | 意味 |
|---|---|
| `outage_started` | 設定を失わない一時障害で、効いている対策が 0 件になり一般登録が一時拒否になった |
| `outage_recovered` | 上記から復旧した |
| `protection_degraded` | 片方の対策だけ障害 (防御低下)。動作中の対策のみで登録は継続 |
| `protection_lost_stopped` | 両対策の設定が失われ、停止ラッチが成立した (復旧しても管理者の手動再開が必要) |
| `override_started` | 管理者が override (保護なしの一般開放) を開始した |
| `override_revoked` | 管理者が override を解除した |
| `webhook_test` | 管理者のテスト送信 (合成イベント。event ID は `test_` で始まる。通知一覧・ゲート状態には現れない) |

### 本文 (JSON)

allowlist 方式の固定スキーマ。**登録者のメールアドレス・secret・操作した管理者・内部エラー詳細は含まれない。**

```json
{
  "version": 1,
  "event_id": "123456789",
  "type": "outage_started",
  "occurred_at": "2026-11-10T01:02:03.456789Z",
  "gate": {
    "admin_state": "open",
    "override_active": false,
    "config_lost_latch": false,
    "episode": "outage",
    "revision": 7
  },
  "email_verification": { "configured": true, "probe": "unhealthy" },
  "bot_protection": { "configured": false, "probe": "unknown" }
}
```

- `gate` はそのイベントを生んだ遷移 **直後** の制御状態の要約 (`admin_state`: `open`/`closed`、`episode`: `none`/`outage`/`degraded`)。
  `email_verification` / `bot_protection` は設定上の有無と検証 API の観測 (`probe`: `healthy`/`unhealthy`/`unknown`)。
- `webhook_test` の本文は `gate` などを持たない (`version`/`event_id`/`type`/`occurred_at` のみ)。
- フィールドは将来追加される可能性がある。受信側は未知のフィールドを無視すること。互換性を壊す変更では `version` を上げる。

## 3. 署名の検証

署名対象は **`"{X-Emumet-Timestamp}." + 受信した本文のバイト列`** (本文は再シリアライズせず、受信したバイト列のまま使う)。

1. `X-Emumet-Timestamp` を整数として解釈し、現在時刻との差が **許容時刻差 (推奨 5 分)** 以内か確認する。超えたら拒否 (リプレイ緩和)。
2. `HMAC-SHA256(secret, timestamp + "." + body)` を計算し、小文字 hex にする。
3. `X-Emumet-Signature` の `v1=` 以降と **定数時間比較** (タイミング攻撃対策。通常の `==` を使わない) で比較する。
4. 一致しなければ 4xx で拒否する (※ 4xx は Emumet 側で恒久失敗になる。署名検証の誤設定は管理者が気付けるようにするため)。

### テストベクタ

```
secret    = whsec_test_secret_0123456789abcdef
timestamp = 1700000000
body      = {"version":1,"event_id":"123456789","type":"outage_started"}
signed    = 1700000000.{"version":1,"event_id":"123456789","type":"outage_started"}
signature = v1=153d6760b728623755e694d34f231eef1aa23a335378c41fec53ac2d05a19771
```

(この値は `driver/src/webhook/tests.rs` の `signature_matches_the_documented_test_vector` が固定している。perl の `Digest::SHA::hmac_sha256_hex` でも再現できる。)

### 検証例 (Python)

```python
import hashlib, hmac, time

def verify(secret: bytes, body: bytes, timestamp: str, signature: str, tolerance: int = 300) -> bool:
    if abs(time.time() - int(timestamp)) > tolerance:
        return False
    expected = "v1=" + hmac.new(secret, timestamp.encode() + b"." + body, hashlib.sha256).hexdigest()
    return hmac.compare_digest(expected, signature)
```

## 4. 配送保証

- **at-least-once**。同じ event が **複数回届き得る** (再試行、受信後に応答が届かなかった場合、crash 後の再送、管理者の再試行)。
  受信側は **`X-Emumet-Event-Id` (= 本文の `event_id`) で重複排除**すること。同じ event の本文は再送でも同一。
- **順序は保証しない**。必要なら `occurred_at` (または `gate.revision`) で整列する。
- 同じ event を複数の worker/プロセスが同時に送ることはない (lease による排他。lease は送信の直前に 1 件ずつ取り、送信の全体 timeout より長い)。lease 期限切れ (worker の停止) の event は別のプロセスが回収して再送する。
- 再試行待ち (`retrying`) のまま最大期間 (24 時間) を超えた配送は、worker が停止していた後に再開しても **再送せず `failed`** にする (管理者が再試行を指示すれば送る)。一度も試行していない配送は、停止期間が長くても再開後に送る。
- **Webhook を有効にする前に発生したイベントは配送しない**。有効と最初に観測された時点までのイベントは配送対象外
  (管理画面の通知一覧には残る)。以降のイベントは、worker 停止・無効化・設定不備の期間をまたいでも取りこぼさず配送する。
- 配送は登録ゲートの状態遷移の transaction の外側で動く。**Webhook の不達・遅延・設定不備・worker 停止でゲートの判定・状態・通知イベントの生成は変わらない。**
  配送の失敗・failed・設定不備は新しい通知イベント (= Webhook 配送対象) を生まない (状態 API で観測する)。

## 5. 応答と再試行

受信側は 2xx を速やかに返すこと (全体 timeout は 15 秒)。応答本文は読み捨てられ記録されない。

| 結果 | 扱い |
|---|---|
| 2xx | 成功 (`delivered`) |
| 408 / 429 / 5xx / timeout / 接続失敗 (DNS 失敗を含む) | 再試行 (指数バックオフ 30 秒から最大 1 時間 + jitter。429/503 の `Retry-After` は最大 1 時間まで尊重)。1 サイクルあたり最大 8 回、または 24 時間を超えたら `failed` |
| 3xx | リダイレクトは追従せず恒久失敗 (`failed`) |
| その他の 4xx、宛先検査 (SSRF) の拒否、設定不備 | 恒久失敗 (`failed`)。再試行しない |

`failed` は管理者が再試行を指示するまで再送されない。再試行は **同じ event ID** を再送する。

## 6. 宛先の制約 (SSRF 対策)

- HTTPS のみ。リダイレクトは追従しない。URL に認証情報は含められない。
- 送信ごとに DNS を解決し、**解決された全アドレス**を検査する。loopback / private / link-local / CGNAT / multicast / unspecified /
  IPv4-mapped IPv6 / ULA / クラウド metadata (`169.254.169.254` 等) / NAT64 / 6to4 など、グローバル unicast でないアドレスが 1 つでも含まれれば拒否 (恒久失敗)。
- 検査したアドレスへ接続を固定する (接続時に再解決しない = DNS rebinding 対策)。TLS 証明書は元のホスト名で検証する。
- 環境変数の proxy 設定は使わない。
- テスト用に http/内部宛先を許す設定は **本番ビルドの設定からは有効化できない** (テストビルド/`test-mode` feature のコードにのみ存在する)。
- したがって、社内ネットワーク内の受信サーバー (private IP) へは直接配送できない。公開 HTTPS endpoint (リバースプロキシ等) を用意すること。

## 7. 運用 (管理者 API)

すべて管理者 (`instance:administrate`) のみ。非管理者は 401/403。secret はどの応答にも含まれない。詳細は `openapi.json` を参照。

| API | 内容 |
|---|---|
| `GET /api/v1/admin/registration-gate/webhook` | 設定状態 (`not_configured` / `misconfigured` + `issue` / `configured`)、宛先 host、直近の成功/失敗時刻、状態別件数、直近の配送記録 (event ID・種別・状態・試行回数・結果分類) |
| `POST /api/v1/admin/registration-gate/webhook/test` | 合成イベント `webhook_test` を同じ経路 (署名・宛先検査・timeout) で 1 回送り、結果を返す。再試行せず、配送記録・通知一覧・未読件数・ゲート状態には影響しない。未設定/設定不備は 409 |
| `POST /api/v1/admin/registration-gate/webhook/retry` | `{"event_id": "..."}` でその failed を、`{}` で全 failed を再キューする (同じ event ID を再送)。指定した event が failed でなければ 404 |

運用手順の例: 受信側を構築 → サーバー設定を投入して再起動 → `GET .../webhook` が `configured` であることを確認 → `POST .../webhook/test` で疎通と署名検証を確認 →
以後の障害通知を受信。`failed` が出たら原因 (`last_result` の分類) を直し、`POST .../webhook/retry` で再送する。
