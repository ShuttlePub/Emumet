# 登録ゲート通知の外部 Webhook (D15 / E15w / E15x)

Emumet は、登録ゲートの通知イベント (障害・復旧・防御低下・停止・override の開始/解除) を、管理画面を開いていなくても届くように
**管理者が登録した複数の HTTPS 送信先** へ署名付きで配送できる。この文書は **受信側** (Webhook を受け取るシステム) と運用者向けの契約。
送信先の登録・編集・削除・状態・テスト送信・再試行は管理者 API (§7) で行う (E15x)。E15w の「環境変数の 1 件だけ」の運用からの移行は §1.4。

## 1. 送信先の管理と設定

### 1.1 送信先 (管理者 API で管理)

- 管理者が送信先を **複数** (上限 **10 件**。超過は 409 `endpoint_limit_reached`) 登録できる。各送信先は URL・署名 secret・送る通知カテゴリ (§2.2)・revision を持つ。
- URL は HTTPS のみ・userinfo と fragment は不可・2048 文字以内。登録時に宛先検査 (§6) を通す。正規化後 (host の小文字化・末尾ドット除去・既定ポート省略) に同じ URL は重複として拒否 (409 `duplicate_url`)。
- **URL・path・query・secret は API 応答に一切返らない**。一覧・取得は部分マスク形 (`masked_url`) だけを返す。例: `hooks.example.net/••••/a8f2`
  (host とポート、path があるときは伏せ字 + **12 文字以上**の最後の path トークンの末尾 4 文字。トークンが 12 文字未満なら `hooks.example.net/••••`。query・userinfo は出さない)。
  そのため編集画面は URL・secret 欄を **空で開き**、入力したときだけ置き換える (PUT で省略した項目は変更しない)。
- **URL は送信に必要なため DB に平文で保存する** (暗号化するのは secret だけ)。path・query に認証トークンを含む URL を使う場合、そのトークンは DB 上では保護されない (DB のバックアップ・アクセス権限で保護すること)。API 応答・監査・ログには全 URL を出さない。
- 更新・削除は **revision 楽観ロック** (リクエスト本文の `revision`。不一致は 409。成功のたびに増える)。削除すると、その送信先の未完了配送・配送履歴は消え、以後その送信先へは配送されない。
- すべての操作 (作成・更新・削除・テスト送信・再試行・環境変数の取り込み) は監査テーブル (`registration_gate_webhook_endpoint_audit`) に操作者・対象送信先 ID・操作種別・結果・revision だけを記録する。URL・secret・通知本文は残さない。
- 送信先の作成・変更・削除・配送失敗・監査は **新しい通知イベント (= Webhook 配送対象) を生まない**。

### 1.2 署名 secret の保存と暗号鍵 (運用者向け)

secret は **書き込み専用**: 作成・更新でのみ受け取り (16〜256 バイト)、API 応答・ログ・エラー・監査・通知・payload のどこにも出ない。DB には **暗号文だけ**を保存する
(AES-256-GCM。`aes-gcm` crate。nonce は暗号化ごとにランダム、関連データ (AAD) に送信先 ID を束縛するので別の送信先の行へ移しても復号できない)。
暗号文には鍵識別子 (`kf-` + 鍵の SHA-256 先頭 4 バイト hex) を添える。

| 環境変数 | 内容 |
|---|---|
| `REGISTRATION_WEBHOOK_ENCRYPTION_KEY` | 暗号鍵 (base64 の **32 バイト**)。 |
| `REGISTRATION_WEBHOOK_ENCRYPTION_KEY_FILE` | 鍵をファイルから読む場合のパス (secret ファイル運用向け)。`ENCRYPTION_KEY` と同時指定は不備。 |

- **生成**: `openssl rand -base64 32` で作る。リポジトリ・DB・ログ・チャットに貼らない。**保管**: secret ファイル (Kubernetes Secret 等) か secret manager に置き、DB のバックアップと **別の場所**に保管する (DB と鍵が一緒に漏れると暗号化の意味がなくなる)。
- **鍵が未設定 (または不備) の間**: 送信先の **作成と secret の更新は 409 `encryption_key_not_configured` で拒否**する (secret を平文で保存しない)。URL・カテゴリの更新、削除、テスト送信 (環境変数の送信先)、再試行はできる。
  既存の送信先の配送は、保存済み secret を **復号できる限り続く**。復号できない送信先の配送は、その送信先だけが `secret_undecryptable` で恒久失敗 (`failed`) になる (他の送信先とゲートの状態には波及しない)。
- **鍵を失う・誤る・変える**と、保存済み secret は復号できない。該当する送信先は状態が `failing` (理由 `secret_undecryptable`) になり、配送は `secret_undecryptable` で恒久失敗する。
  **復旧手順**: 正しい鍵を設定して再起動する (元の鍵が戻るなら secret の再入力は不要で、`failed` を再試行 (§7) すれば再送される)。元の鍵が失われているなら、
  各送信先で **secret を再入力** (PUT で `secret` を指定。現在の鍵で暗号化し直される) し、`failed` を再試行する。
- **鍵のローテーションは未対応** (暗号文は鍵識別子を持つので将来対応できる形だが、ローテーション機構・二重鍵での復号は無い)。鍵を変えると全送信先の secret を再入力することになる。署名 secret のローテーション (二重受理) も無い。
- 環境変数: `REGISTRATION_WEBHOOK_POLL_INTERVAL_MS` は配送 worker の poll 間隔 (既定 2000)。

### 1.3 配送 worker とゲートの分離

配送 worker は常に起動する (送信先は API で管理されるため。送信先がなければ何もしない)。送信先ごとに **独立** に処理するので、ある送信先の遅延・失敗・復号不可は他の送信先の配送を止めない。
どの送信先の不達・遅延・設定不備・worker 停止・送信先の登録/編集/削除でも、ゲートの判定・状態・通知イベントの生成は変わらない。

### 1.4 既存の環境変数 1 件 (E15w) の移行

E15w の環境変数 (`REGISTRATION_WEBHOOK_URL` / `REGISTRATION_WEBHOOK_SECRET` または `_SECRET_FILE`) で運用中の環境が、アップグレードだけで配送を失うことはない。

- **暗号鍵あり (§1.2)**: 起動時に、環境変数の送信先が有効 (設定不備でない) で未取り込みなら、**初期送信先**として DB へ **一度だけ**取り込む (全 4 カテゴリ、secret は暗号化して保存)。
  E15w の「有効化前のイベントは配送しない」ベースラインと、既存の未完了 (pending/retrying)・failed の配送記録・試行履歴は **event ID を変えずに** 取り込んだ送信先へ引き継がれる。
  取り込みは冪等 (二重起動・再起動で重複しない)。取り込み後は **DB が正本** で環境変数は無視される: 管理者が編集・削除でき、**削除しても再取り込みしない** (取り込み済みマーカー `registration_gate_webhook_env_import`)。
  取り込みの記録・ログに URL・secret は出ない。
- **暗号鍵なし**: 取り込まない。環境変数の 1 件は従来どおり配送され、送信先一覧に **読み取り専用** の `id = environment`・`source = environment`・`editable = false` として現れる
  (編集・削除は 409 `environment_endpoint_read_only`。テスト送信・再試行・failed 一覧・状態は使える)。**移行手順**: 鍵を生成して設定し (§1.2)、再起動する。
- 環境変数が **未設定**なら Webhook の環境変数側は無効で (エラーではない)、一覧に現れない。**設定不備** (http・内部アドレス・secret 欠落/短すぎる等) は起動失敗にならず、
  `environment` の送信先が `status = failing`・`config_issue` (分類のみ) で現れ、配送は行われず、ゲートの動作には影響しない。取り込みもされない。
- migration (`20261123000001_registration_webhook_endpoints.sql`) は E15w の配送記録を **消さず**、event ID も変えない。既存の記録は `endpoint_id = 0` (環境変数の送信先) になり、上記のとおり取り込み時に付け替わる。

### 1.5 E15w の集約 API の扱い

E15w の `GET/POST test/POST retry /api/v1/admin/registration-gate/webhook*` (単一送信先前提の集約 API) は、frontends に消費者がいないため **送信先単位の API に置き換えた**
(`GET /webhook` と `POST /webhook/test` は削除。`POST /webhook/retry` は同じパスで本文を `{endpoint_id?, event_id?}` に変更)。
E15w の運用手順 (状態確認 → テスト送信 → failed の再試行) は §7 の API で一貫して行える (環境変数の送信先は `id = environment`)。状態 API の「直近 50 件」の制限は、failed 一覧の cursor ページング (§7) で解消した。

## 2. リクエスト

`POST <送信先 URL>`、`Content-Type: application/json`。リダイレクトは **追従しない** (3xx は恒久失敗)。

### ヘッダー

| ヘッダー | 内容 |
|---|---|
| `X-Emumet-Event-Id` | **安定した event ID**。再試行・crash 後の再送・管理者の再試行でも同一。重複排除のキー。 |
| `X-Emumet-Event-Type` | イベント種別 (下表)。 |
| `X-Emumet-Delivery-Attempt` | その event の通算試行番号 (1 始まり。管理者の再試行をまたいで増える)。 |
| `X-Emumet-Timestamp` | 送信時刻 (Unix 秒)。**試行ごとに更新される**。 |
| `X-Emumet-Signature` | `v1=<小文字 hex>` の HMAC-SHA256 署名 (§3)。**送信先ごとの secret** で作る。 |

### 2.1 イベント種別

| `type` | 意味 |
|---|---|
| `outage_started` | 設定を失わない一時障害で、効いている対策が 0 件になり一般登録が一時拒否になった |
| `outage_recovered` | 上記から復旧した |
| `protection_degraded` | 片方の対策だけ障害 (防御低下)。動作中の対策のみで登録は継続 |
| `protection_lost_stopped` | 両対策の設定が失われ、停止ラッチが成立した (復旧しても管理者の手動再開が必要) |
| `enforcement_lost_stopped` | 本番の登録ブロック (Kratos 側の強制) を確認できなくなり、一般登録を自動停止した (override より優先。復旧しても管理者の手動再開が必要) |
| `enforcement_restored` | 登録ブロックの確認が復旧した (停止は継続。手動再開が必要) |
| `override_started` | 管理者が override (保護なしの一般開放) を開始した |
| `override_revoked` | 管理者が override を解除した |
| `webhook_test` | 管理者のテスト送信 (合成イベント。event ID は `test_` で始まる。通知一覧・ゲート状態には現れない) |

### 2.2 送る通知カテゴリ

送信先ごとに、次の **4 カテゴリ** から 1 つ以上を選ぶ (識別子は API で固定。`webhook_test` は選択の対象外で、テスト送信でのみ送られる)。
`GateEventKind` との対応は **ワイルドカードなしの網羅 match** で実装されており、新しい種別を追加するとコンパイルまたはテストで分類を迫られる。

| カテゴリ (UI) | 識別子 | 含まれる `type` | 管理画面の通知タイトル |
|---|---|---|---|
| 障害 | `outage` | `outage_started`、`protection_degraded` | 「一般登録を一時停止しました」「{保護名}が応答しません」 |
| 復旧 | `recovery` | `outage_recovered`、`enforcement_restored` | 「{保護名}が復旧しました」「保護の適用が復旧しました」 |
| 自動停止 | `auto_stop` | `protection_lost_stopped`、`enforcement_lost_stopped` | 「一般登録を自動停止しました」 |
| 強制開放 | `override` | `override_started`、`override_revoked` | 「強制開放しました」「強制開放を解除しました」 |

- 配送記録は、**イベント発生時点でその送信先がそのカテゴリを購読していた場合だけ**作る。選択の変更は、変更後に発生したイベントにだけ効く
  (変更の時点で、変更前の選択で未取り込みだったイベントを先に取り込み、それ以外の過去のイベントは新しい選択でも配送しない)。すでに作られた pending/retrying/failed の配送記録は選択の変更では消えず、そのまま配送・再試行される。
- **新しい送信先は作成後に発生したイベントだけ**を配送する (過去イベントの遡及配送はしない)。

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

- `gate.enforcement_lost_latch` は `enforcement_lost_stopped` による停止ラッチが成立しているときだけ現れる (`true`)。
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
- **event ID は送信先をまたいで同一** (通知イベントの ID)。送信先ごとに別の配送記録・署名・再試行状態を持ち、互いに独立する。
- **順序は保証しない**。必要なら `occurred_at` (または `gate.revision`) で整列する。
- 同じ event を複数の worker/プロセスが同時に送ることはない (lease による排他。lease は送信の直前に 1 件ずつ取り、送信の全体 timeout より長い)。lease 期限切れ (worker の停止) の event は別のプロセスが回収して再送する。
- 再試行待ち (`retrying`) のまま最大期間 (24 時間) を超えた配送は、worker が停止していた後に再開しても **再送せず `failed`** にする (管理者が再試行を指示すれば送る)。一度も試行していない配送は、停止期間が長くても再開後に送る。
- **送信先を作る (または環境変数の送信先を有効にする) 前に発生したイベントは配送しない** (管理画面の通知一覧には残る)。以降のイベントは、worker 停止・設定不備の期間をまたいでも取りこぼさず配送する。
- 配送は登録ゲートの状態遷移の transaction の外側で動く。**Webhook の不達・遅延・設定不備・worker 停止でゲートの判定・状態・通知イベントの生成は変わらない。**
  配送の失敗・failed・設定不備は新しい通知イベント (= Webhook 配送対象) を生まない (状態 API で観測する)。
- **プロセスの停止と再起動** (実 DB の回帰テストで固定): 停止要求を受けた worker は次の送信に進まず、**送信中の 1 件は結果の記録まで待つ** (プロセス終了も最大 20 秒待つ)。
  未送信の配送は due のまま残り、再起動後に 1 回だけ送られる (正常な停止・再起動で同じ event を二重に送らない)。送信後・記録前にプロセスが強制終了した場合だけ、lease の失効後に **同じ event ID** で再送される (at-least-once)。

## 5. 応答と再試行

受信側は 2xx を速やかに返すこと (全体 timeout は 15 秒)。応答本文は読み捨てられ記録されない。

| 結果 | 扱い |
|---|---|
| 2xx | 成功 (`delivered`) |
| 408 / 429 / 5xx / timeout / 接続失敗 (DNS 失敗を含む) | 再試行 (指数バックオフ 30 秒から最大 1 時間 + jitter。429/503 の `Retry-After` は最大 1 時間まで尊重)。1 サイクルあたり最大 8 回、または 24 時間を超えたら `failed` |
| 3xx | リダイレクトは追従せず恒久失敗 (`failed`) |
| その他の 4xx、宛先検査 (SSRF) の拒否、設定不備、secret を復号できない (`secret_undecryptable`) | 恒久失敗 (`failed`)。再試行しない |

`failed` は管理者が再試行を指示するまで再送されない。再試行は **同じ event ID** を再送する。

**送信先の状態**: `healthy` (正常) / `failing` (配送失敗)。`failing` = その送信先に **未解決の failed 配送がある**、または復号不可 (`secret_undecryptable`)・環境変数の設定不備 (`config_invalid`) などの恒久的な理由がある。
failed が再試行で解消されれば (再送が成功して failed でなくなれば) `healthy` に戻る。テスト送信は状態に影響しない。

## 6. 宛先の制約 (SSRF 対策: 登録時と接続時の二段)

- **登録時** (作成・URL 更新): HTTPS 限定・userinfo/fragment 拒否・host の正規化と検査 (10 進/16 進/短縮 IPv4 などの数値表記、IPv6 ゾーン、末尾ドット、`localhost` 系)・リテラル IP の検査・名前解決結果の **全アドレス** の検査。
  拒否は 400 (固定コード: `invalid_url` / `unsupported_scheme` / `credentials_in_url` / `fragment_in_url` / `missing_host` / `url_too_long` / `forbidden_destination` / `host_unresolvable`)。どの内部アドレスに解決したかなどの詳細は返さない。
- **接続時** (送信のたびに毎回): 以下。登録時に通った送信先でも、接続時に内部アドレスへ変わっていれば拒否され、恒久失敗 (`ssrf_blocked`) として記録される。
- HTTPS のみ。リダイレクトは追従しない。URL に認証情報は含められない。
- 送信ごとに DNS を解決し、**解決された全アドレス**を検査する。loopback / private / link-local / CGNAT / multicast / unspecified /
  IPv4-mapped IPv6 / ULA / クラウド metadata (`169.254.169.254` 等) / NAT64 / 6to4 など、グローバル unicast でないアドレスが 1 つでも含まれれば拒否 (恒久失敗)。
- 検査したアドレスへ接続を固定する (接続時に再解決しない = DNS rebinding 対策)。TLS 証明書は元のホスト名で検証する。
- 環境変数の proxy 設定は使わない。
- テスト用に http/内部宛先を許す設定は **本番ビルドの設定からは有効化できない** (テストビルド/`test-mode` feature のコードにのみ存在する)。
- したがって、社内ネットワーク内の受信サーバー (private IP) へは直接配送できない。公開 HTTPS endpoint (リバースプロキシ等) を用意すること。

## 7. 運用 (管理者 API)

すべて管理者 (`instance:administrate`) のみ。非管理者は 401/403。secret はどの応答にも含まれず、URL はマスク形でしか返らない。詳細は `openapi.json` を参照。

| API | 内容 |
|---|---|
| `GET /api/v1/admin/registration-gate/webhook/endpoints` | 送信先の一覧 (マスク URL・カテゴリ・revision・状態 `healthy`/`failing`・failed 件数・配送件数・最終成功/失敗時刻)、全体で failed があるか (`has_failed_deliveries`)、暗号鍵の有無、上限 |
| `POST .../webhook/endpoints` | 送信先の作成 (201)。`{url, secret, categories}`。400 (入力不正・固定コード)、409 (`duplicate_url` / `endpoint_limit_reached` / `encryption_key_not_configured`) |
| `GET .../webhook/endpoints/{endpoint_id}` | 送信先 1 件 (一覧と同じ形)。`environment` は環境変数の送信先 (read-only) |
| `PUT .../webhook/endpoints/{endpoint_id}` | 更新。`{revision, url?, secret?, categories?}`。省略した項目は変更しない。revision 不一致は 409 |
| `POST .../webhook/endpoints/{endpoint_id}/delete` | 削除。`{revision}`。revision 不一致は 409。revision を本文で受けるため HTTP DELETE ではなく POST |
| `POST .../webhook/endpoints/{endpoint_id}/test` | 合成イベント `webhook_test` を **その送信先の署名・宛先検査・timeout** で 1 回送り、結果を返す。再試行せず、配送記録・通知一覧・未読件数・ゲート状態・送信先の状態には影響しない (監査だけ記録)。環境変数の送信先が未設定/設定不備なら 409 |
| `POST .../webhook/retry` | `{endpoint_id, event_id}` で (送信先, event ID) 1 件の failed を、`{endpoint_id}` でその送信先の failed 全件を、`{}` で全送信先の failed 全件を再キューする (同じ event ID を再送)。failed でなければ 404、`event_id` だけの指定は 400 |
| `GET .../webhook/failures?endpoint_id=&limit=&before=` | failed の配送を **cursor ページング** で返す (failed になった時刻の新しい順 + 送信先 ID + event ID の降順で決定的。`next_cursor` を `before` へ渡す)。件数に関わらず **古い failed にも到達して 1 件ずつ再試行できる**。成功した配送は出さない。各項目は送信先 ID・event ID・種別・カテゴリ・失敗時刻・結果分類・失敗理由 |

運用手順の例: 暗号鍵を設定 (§1.2) → 受信側を構築 → `POST .../webhook/endpoints` で送信先を登録 → `POST .../endpoints/{id}/test` で疎通と署名検証を確認 →
以後の障害通知を受信。一覧の状態が `failing` になったら `GET .../webhook/failures` で原因 (`result` の分類) を確認・修正し、`POST .../webhook/retry` で再送する。
