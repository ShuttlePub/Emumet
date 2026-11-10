# 登録保護の readiness と登録前 email proof (D15 / E15c)

登録ゲート (E15a) の観測入力を**実際の検証 API の結果**から作るための adapter と、登録者のメール所有を確認する
登録前 email proof、Turnstile Siteverify adapter、管理者の実検証 API をまとめる。保護の所在は host ADR
`registration-protection-placement` (2026-11-10 Accepted) のとおりで、メールは「Emumet が登録前に proof を発行・検証し password 登録を維持」、
BOT は「Emumet の webhook + Turnstile Siteverify」。

**この unit に含まれないもの (E15d の所有)**: 本番 Kratos registration hook への配線・hook 認証、継続 probe の周期実行 (scheduler)、
hook 時の最新評価、config attestation、Kratos 設定変更。この unit は「1 回の probe を実行して記録する use case」「時刻・generation を引数に
readiness から観測入力を導く評価」「proof の照合・消費 (内部 use case)」を提供する。**health・ファイルの存在・HTTP 200・SMTP 250 だけでは
`operational` にならない。古い成功を無期限に使わない。**

## 1. 設定 (サーバー側のみ。secret は DB に保存しない)

| 環境変数 | 内容 |
|---|---|
| `REGISTRATION_TURNSTILE_SITE_KEY` | サイトキー (非 secret。generation に含まれる)。 |
| `REGISTRATION_TURNSTILE_SECRET` / `_SECRET_FILE` | Siteverify の secret。両方指定は設定不備 (`invalid_configuration`)。 |
| `REGISTRATION_TURNSTILE_EXPECTED_HOSTNAME` | **必須**。Siteverify 応答の `hostname` と照合する。 |
| `REGISTRATION_TURNSTILE_EXPECTED_ACTION` | 任意。設定すると応答の `action` と一致を要求する。 |
| `REGISTRATION_TURNSTILE_TOKEN_MAX_AGE_SECS` | `challenge_ts` からの許容秒数 (既定 300、1〜3600)。 |
| `REGISTRATION_TURNSTILE_ENABLED` | `false`/`0`/`off`/`no`/`disabled` で明示無効 (`disabled`)。 |
| `REGISTRATION_SMTP_HOST` / `_PORT` | SMTP サーバー。ポートの既定は STARTTLS=587、`tls`=465。 |
| `REGISTRATION_SMTP_TLS` | `starttls` (既定。STARTTLS を必須にし、平文へ降格しない) または `tls` (implicit TLS)。**平文は本番の読込では選べない** (テストビルドのみ)。 |
| `REGISTRATION_SMTP_USERNAME` / `_PASSWORD` / `_PASSWORD_FILE` | 認証。username と password は両方指定するか、両方省略 (認証なしの内部 relay)。 |
| `REGISTRATION_EMAIL_FROM` | 差出人 (`no-reply@example.com` または `Name <addr>`)。 |
| `REGISTRATION_EMAIL_CANARY_ADDRESS` | 承認済み canary mailbox (管理者の roundtrip 検証用)。未設定ならメールは `unverified` のまま。 |
| `REGISTRATION_EMAIL_ENABLED` | 明示無効。 |
| `REGISTRATION_PROTECTION_SUCCESS_TTL_SECS` | business success の有効期間 (既定 86400)。 |
| `REGISTRATION_PROTECTION_PROBE_MAX_AGE_SECS` | 最後の検証からの許容秒数 (既定 600)。 |
| `REGISTRATION_PROOF_REUSE_WINDOW_SECS` | proof の transport retry 再利用窓 (既定 60)。 |
| `REGISTRATION_EMAIL_MAX_PER_SOURCE_HOUR` | 送信元単位の 1 時間あたり上限 (既定 20)。 |

- **すべて未設定は `configuration_invalid` (`not_configured`)** で、起動失敗ではない。設定不備 (hostname 欠落・不正な host/port/from・secret 欠落・
  username だけ指定など) も起動失敗にならず、readiness が `configuration_invalid` と理由 (`invalid_configuration` / `missing_secret` ...) を返す。
- secret・SMTP パスワードは DB・API 応答・ログ・エラー・通知のどこにも出ない。`Debug` も伏せる。

### 設定世代 (generation)

保護ごとに、provider・enabled・非 secret の設定値 (host/port/TLS/username/from/canary、site key/hostname/action/期限) と secret の
fingerprint から **決定的に** 導く (SHA-256 の先頭 128 bit。値は長さ接頭辞つきで連結)。設定が 1 つでも変われば generation が変わり、
旧 generation の **readiness の成功・challenge・proof はすべて無効** になる。secret の `fingerprint` はドメイン分離した SHA-256 の先頭 48 bit
(非 secret として API に出る)。**弱い secret は総当たりで照合され得る**ので、十分長いランダム値を使うこと。

## 2. readiness の意味と評価

`GET /api/v1/admin/registration-gate/protections` の `status` は **その時刻・現在の設定で毎回再評価**した値 (DB の記録は評価の素材)。

| status | 意味 |
|---|---|
| `operational` | 当該 generation に束縛された business success が鮮度内。**E15a では configured=true・Healthy** |
| `temporarily_unavailable` | 設定は失っていない一時障害 (429/5xx/timeout/接続失敗/不正応答/SMTP 4xx)。configured=true・Unhealthy。同じ範囲の成功で戻る (時間経過だけでは戻らない) |
| `unverified` | 未検証・設定変更後 (`generation_changed`)・成功期限切れ (`success_expired`)・probe 停止 (`probe_stale`、**既知の障害が無いときだけ**)・business success なし (`no_business_success`)。configured=true・Unknown |
| `configuration_invalid` | 未設定・空設定・明示無効・無効な secret・SMTP 認証/設定/TLS 不正。**configured=false**。旧成功は使わない。両保護がこの状態なら E15a が設定喪失として停止 latch |

評価 (`kernel::evaluate_readiness`, 時刻は引数):

1. 設定が使えない → `configuration_invalid`
2. 記録なし / generation 不一致 → `unverified`
3. **保持している既知の障害** (範囲ごと: 接続範囲・配送範囲) があれば、鮮度に関係なく最も悪いものを返す (`configuration_invalid` > `temporarily_unavailable`、同格なら新しい方の理由)
4. 最後の検証が `probe_max_age` 以上前 → `unverified` (probe 停止。**期限ちょうどは期限切れ**)
5. 当該 generation の business success が `success_ttl` 内なら `operational` (`valid_until` = min(成功期限, 最後の検証 + probe_max_age))、そうでなければ `unverified`

### 単調性の原則と網羅表

**より悪い既知状態は、それを解消する証拠 (同じ範囲の成功) か generation の変更でしか改善しない。** 後続のより軽い失敗 (`configuration_invalid` の後の timeout)、時間の経過 (鮮度切れ)、別の範囲の成功では、解除も軽減もされない。
成功が解消する範囲: 接続範囲の成功 (接続 probe・BOT の business success・登録者の code 確認の成功) は **接続範囲の障害だけ**、canary の配送成功は **接続・配送の両方**。失敗は同じ範囲の既存の障害へ「より軽ければ据え置き、同格以上なら最新で置き換え」で畳む。
`attempt_rejected` は何も変えず、記録もしない。

配送範囲の障害 × 最新の接続結果 × 時間・generation の全組合せ (`kernel` の `readiness_is_monotonic_over_every_combination` が期待値を手書きで網羅する。E15a への観測 = (configured, healthy)):

| 保持する配送障害 | 最新の接続結果 | 通常 (鮮度内) | 鮮度切れ | generation 変更 |
|---|---|---|---|---|
| なし | accepted | operational (true, Healthy) | unverified `probe_stale` (true, Unknown) | unverified `generation_changed` (true, Unknown) |
| なし | temporary (timeout 等) | temporarily_unavailable (true, Unhealthy) | 同左 (据え置き) | 同上 |
| なし | invalid (無効 secret 等) | configuration_invalid (**false**) | 同左 (据え置き) | 同上 |
| なし | 未観測 | operational | unverified `probe_stale` | 同上 |
| temporary | accepted | temporarily_unavailable (配送障害の理由) | 同左 (据え置き) | 同上 |
| temporary | temporary | temporarily_unavailable (新しい方の理由) | 同左 | 同上 |
| temporary | invalid | configuration_invalid (**false**) | 同左 | 同上 |
| temporary | 未観測 | temporarily_unavailable | 同左 | 同上 |
| invalid | accepted | configuration_invalid (**false**, 配送障害の理由) | 同左 | 同上 |
| invalid | **temporary** | **configuration_invalid (false)**。timeout で一時障害へ軽くならない | 同左 | 同上 |
| invalid | invalid | configuration_invalid (新しい方の理由) | 同左 | 同上 |
| invalid | 未観測 | configuration_invalid | 同左 | 同上 |

両保護が `configuration_invalid` (configured=false) になると E15a の設定喪失の停止 latch が成立し、復旧しても自動では再開しない (管理者の手動再開)。この性質を、既知の設定喪失が後続の timeout で観測上消えないこと、で守る (application の実 DB テスト `a_known_configuration_loss_survives_a_later_timeout_and_the_latch_needs_a_manual_resume`)。

`accepted` は「資格情報・接続が受理された」(SMTP 250・負の probe で secret が受理された等) で、**operational の根拠にならない**。

### 分類表 (ADR の失敗分類の実装)

| 条件 | 分類 | E15a の観測入力 |
|---|---|---|
| 未設定/空/明示無効、invalid/missing secret、SMTP 認証 (530/534/535/538)・認証以外の 5xx・TLS 不正 | `configuration_invalid` | configured=false |
| token 不正・duplicate・hostname/action 不一致・期限切れ token、**公開要求の送信 (`SendPurpose::Registration`) で宛先提示後 (RCPT コマンド送信以降) に起きた SMTP の失敗すべて** (RCPT・DATA の全コード・その段階の timeout/切断。550-554 は `recipient_rejected`、それ以外は `recipient_deferred`)。管理者 canary では同じ失敗を下の provider 障害として扱う | `attempt_rejected` | **観測を変えない (記録もしない)** |
| HTTP 429/5xx、timeout/DNS/connect/TLS、不正な provider 応答、SMTP の接続失敗/timeout/4xx/421 (宛先提示前の段階、または **管理者 canary の全段階** (RCPT・DATA・timeout・切断を含む)) | `temporarily_unavailable` | configured=true・Unhealthy |
| business success + generation/鮮度一致 | `operational` | configured=true・Healthy |
| 設定変更後/成功期限切れ/probe 停止 | `unverified` | configured=true・Unknown |

- HTTP provider の 429/5xx と SMTP の全 5xx は同一視しない。SMTP は失敗した**段階** (接続・認証・MAIL FROM・RCPT・DATA) を保持して分類し、**分類は送信の目的で分ける**: 公開要求 (`Registration`) では宛先提示後 (RCPT コマンド送信以降) の失敗をコード・段階・timeout・切断を問わずすべて attempt_rejected とし、provider の障害を示せるのは宛先提示前 (接続・STARTTLS・認証・MAIL FROM) の失敗だけ。管理者 canary (`Canary`、固定の承認済み宛先で攻撃者が選べない) では全段階の失敗を provider の障害として分類する (4xx・timeout・切断は一時障害、認証失敗・5xx (canary 宛先の 550 系を含む)・設定不備は設定不正)。MAIL FROM の拒否・DATA の 5xx などの「configuration_invalid」は canary と宛先提示前の段階での分類である。MAIL FROM の 550 (送信者不許可)・DATA の 5xx・接続/認証段階の 55x は宛先拒否ではなく `configuration_invalid` (readiness を健全のままにしない)。
- 一方の保護が故障しても他方は独立に評価される (`observation_from_readiness` は保護ごとに導く)。

## 3. bootstrap と継続検証の設計

**operational の唯一の根拠は business success**。bootstrap (未検証からの初回) と継続検証 (有効期限内の維持) は次のとおり。

### bot (Turnstile)

- **bootstrap**: 管理者が、サイトキーの widget が発行した**有効な token** を `POST .../protections/bot/verify` `{"token": "..."}` に渡す。Siteverify の `success:true` かつ
  hostname/action/token の期限が合致して初めて `business_success` が当該 generation に記録され `operational` になる。dummy token・空 response の失敗だけでは operational にならない。
- **継続検証**: token を伴わない**負の probe** (`verify` を `{}` で呼ぶ / `run_registration_protection_probe`)。有効でない固定 token を送り、provider が secret を受理するか
  (`invalid-input-response` 等 → `accepted`)、secret が拒否されるか (`invalid-input-secret` → `configuration_invalid`)、一時障害か (429/5xx/timeout → `temporarily_unavailable`) を判定する。
  probe は `accepted` なので **operational 自体は business success の鮮度 (`success_ttl`) で維持**される。
- 実際の登録者の token の検証 (`verify_registration_bot_token`、hook から呼ぶ内部 use case) は、business success を記録して鮮度を更新し、provider 起因の失敗も記録する。
  登録者の不正 token (`attempt_rejected`) は記録しない。

### email (SMTP)

- **bootstrap**: `POST .../protections/email/verify` `{}` が (1) 接続・認証 probe (メールは送らない) (2) 承認済み canary mailbox へ実際の code を送信、を行い `pending_confirmation` と
  `challenge_id` を返す。管理者が canary mailbox に届いた code を `{"challenge_id": "...", "code": "..."}` で確認すると、**実 code の確認 roundtrip が business success** として当該 generation に記録され `operational` になる。
  SMTP の接続・250 では operational にならない。`REGISTRATION_EMAIL_CANARY_ADDRESS` が無ければ `canary_not_configured` を返し、メールは `unverified` のまま。
- **継続検証**: 接続・認証 probe (`run_registration_protection_probe`)。実際の登録者の code 確認の成功 (配送と確認の end-to-end の business success) も鮮度を更新する。
- **検証の範囲 (接続範囲と配送範囲)**: 接続・認証 probe は **接続範囲** だけを確認する。管理者 canary の配送失敗 (RCPT/DATA/timeout/切断を含む) は **配送範囲の障害** として generation ごとに保持され (`delivery_failure`)、**接続 probe の成功 (`accepted`) や登録者の code 確認の成功では解除されない**。operational へ戻る (BOT のみへの縮退が解除される) のは、canary の配送成功だけ。接続範囲の障害 (接続 probe・公開発行の宛先提示前の失敗 = `connection_failure`) は、接続範囲の成功 (または canary の配送成功) まで保持され、後続のより軽い失敗や時間経過では改善しない (上の単調性の原則)。設定変更 (generation 変更) で保持された障害は破棄される。canary の配送受理は周期的に実行できる (周期実行は E15d)。
- canary の確認は **proof を発行せず**、登録者向けの公開確認では使えない。provider の健全性と登録者ごとの所有証明は分離されている。一回の配送成功から全宛先への到達は保証しない。

### 周期実行は E15d

継続 probe を `probe_max_age` (既定 10 分) より短い周期で回すのは E15d (scheduler)。**それまでは bootstrap から 10 分で `unverified` (probe_stale) に戻る**のが仕様どおり (古い成功を使わない)。
成功の有効期間 `success_ttl` (既定 24 時間) は、probe が続いていても成功が古ければ operational にしないための上限。

### 実 credential が無い場合

実 Turnstile 資格情報・実 SMTP アカウント・canary mailbox が無い環境では、その保護は `unverified` (または `configuration_invalid`) のままで effective にならない。公式テストキー・ローカル SMTP 模擬は
protocol と分類の検証であり、**本番 credential の ready 証明ではない**。

## 4. 登録前 email challenge / proof

公開 API (未認証。パスは `openapi.json` が正):

| API | 内容 |
|---|---|
| `POST /api/v1/registration/email-challenges` `{email, flow_id}` | 発行。202 `{challenge_id, expires_at}`。code は 8 桁の数字でメール送信。 |
| `POST /api/v1/registration/email-challenges/{challenge_id}/confirm` `{code}` | 確認。正しい code だけが opaque な `proof` (256 bit 乱数・一度だけ表示) を発行。不明/不一致/期限切れ/試行超過/旧 generation はすべて同じ 400。 |

**proof の照合・消費は公開 HTTP の書込口ではない** (内部 use case `verify_registration_email_proof(flow_id, email, proof)`。hook への配線は E15d)。

- **束縛**: 正規化 email (trim・小文字化・ASCII のみ。複数宛先・制御文字・区切り記号は拒否) + 実 registration flow ID + config generation。DB に残るのは **code・proof・email の hash のみ** (平文・メール本文・secret は保存せずログにも出さない)。
  code と proof の比較は定数時間。code の hash は challenge ID と束縛する。
- **SMTP 送信成功だけでは proof を発行しない**。code の確認後にだけ発行する。送信に失敗した challenge は失効させる (届いていない code を確認させない)。
- **TTL・上限 (既定値と根拠)**:

| 項目 | 既定 | 根拠 |
|---|---|---|
| challenge の有効期間 | 10 分 (flow の期限を超えない) | メール到達と入力に十分で、推測窓を短く保つ |
| 確認の試行上限 | 5 回 (超過で失効) | 8 桁 code (10^8) に対し 1 challenge あたりの当たり確率を 5×10^-8 に抑える |
| proof の有効期間 | 15 分 (flow の期限を超えない) | 確認後に登録フォームを完了する時間。登録 flow より先に切れない |
| proof の再利用窓 | 60 秒 (設定可) | Kratos hook の transport retry (最大 3 呼出を観測) にだけ同じ検証済み事実を返す |
| 送信 rate limit | 同一 email 60 秒に 1 通・1 時間 5 通、同一 flow 1 時間 3 通、同一送信元 1 時間 20 通 | メール爆弾・踏み台の抑止 |

- **原子的な一回性**: 確認・消費はいずれも challenge 行のロック下で判定する。同時の確認は 1 つだけが proof を得る。proof は初回の照合で消費済みになり、**同じ flow/email/proof hash/generation の再利用窓内の再呼出にだけ**
  同じ検証済み事実 (`replayed: true`) を返す。別 flow・別 email・旧 generation・期限切れ・改変・窓の外の再利用は拒否。拒否は proof を消費しない。**gate の許可 verdict 自体は cache しない** (毎回の最新評価は E15d)。
  proof の期限は「期限ちょうどは期限切れ」で、**消費済みの proof でも期限後は再利用窓内であっても retry を成功させない** (proof・flow の期限を消費済み判定より先に確認する)。
- **rate limit の判定と記録は advisory lock で直列化**する (並行発行でも上限を超えない)。集計は要求時刻より後の既存予約も数えるので、要求時刻と予約順が前後しても上限を超えない。送信元は**接続元アドレス**だけを根拠にし (`X-Forwarded-For` 等の信頼できないヘッダーは使わない)、IPv6 は /64 に丸める。
  **リバースプロキシの背後では接続元がプロキシになる**ため、送信元単位の上限はサイト全体の backstop として働く (email/flow 単位の上限が主)。proxy 配下では `REGISTRATION_EMAIL_MAX_PER_SOURCE_HOUR` を調整するか、信頼できる proxy 設定を E15d/deployment で用意すること。
- **公開 API の濫用対策 (gate の参照方針)**: 発行は、**E15a の gate が受付中 (`Accept`) で、かつメール保護が現在 `operational` のときだけ**送る。閉鎖・停止・一時拒否・未検証・設定不正・古い成功のときは 503 で、**任意の宛先へ送信しない**
  (未検証の SMTP を不特定多数の宛先に使わないため)。送信を試みた発行は、受理・宛先拒否 (RCPT 550)・宛先依存の一時拒否 (RCPT 450 等)・provider 障害のいずれでも **同じ 202 応答** で、宛先の状態を区別させない (user enumeration の防止。届かなかった challenge は内部で失効させ、宛先に依存しない provider 障害だけを内部の readiness に記録し、以降の発行を 503 にする。応答時間の差は対象外)、email の正規化後の hash 以外を保持しない。
- **flow の実在確認 (fail-closed)**: Kratos public API の `GET /self-service/registration/flows?id=` で flow の実在と **実際の期限** を確認し、challenge の期限を flow 以内にする。404/410 は不正な flow。
  browser flow は CSRF cookie なしだと 403 になる (Kratos v1.3.1 の実装を確認) ため、発行リクエストに付いてきた Kratos の CSRF cookie (`csrf_token_*` だけ。セッション等の cookie は転送しない) を転送して照会する。
  **cookie がなく期限を確認できない flow は使えないものとして拒否 (400) し、期限を推測・合成しない**。Kratos に到達できなければ 503。フロントエンドは Kratos の cookie が届く構成 (credentials を含めて呼ぶ) で呼ぶこと。
  **最終的な flow 束縛の強制は、hook が Kratos の実 context と proof を照合する側 (E15d)** の責務。
- challenge/送信記録は発行時に、期限を 1 日以上過ぎた行を掃除する。

## 5. Turnstile Siteverify adapter

- `POST https://challenges.cloudflare.com/turnstile/v0/siteverify`。**HTTPS 固定・redirect 追従なし・proxy 不使用**・接続 2 秒/全体 3 秒の timeout (E15d の hook 全体 5 秒以内)・応答本文 16 KiB 上限。
  base URL の差し替えは `cfg(test)` / `test-mode` ビルドにだけ存在し、本番設定から有効化できない。
- 応答の写像 (`classify_siteverify`。E15b の実応答 `e2e/registration-guard/evidence/provider.json` を fixture 化):
  HTTP 200 でも `success:false` は失敗。`missing/invalid-input-secret` → `configuration_invalid`、`invalid/missing-input-response`・`timeout-or-duplicate`・`bad-request` → `attempt_rejected` (provider 全体を障害扱いしない)、
  `internal-error`・429/5xx・timeout・接続/TLS 失敗・不正な応答 (JSON でない・`success` 欠落・未知の error code・3xx・本文超過) → `temporarily_unavailable`。
  `success:true` は hostname (大文字小文字を区別しない)・action (設定時)・`challenge_ts` の鮮度 (未来 60 秒超も不可) がすべて合致して初めて `business_success`。secret 起因と token 起因の error code が同居したら secret を優先。
- 空・過大な token は provider を呼ばずに `attempt_rejected`。secret・token・応答本文はログ・エラー・分類に出さない。
- 公式公開テストキー (`1x…AA` 等) での実 API 検証は `#[ignore]` の `live_turnstile_official_test_keys` (外部ネットワークが必要なため通常は実行しない)。

## 6. SMTP adapter

lettre の `AsyncSmtpConnection` (tokio + native-tls) で、接続 → STARTTLS → 認証 → MAIL FROM → RCPT → DATA の各段階を自前で順に実行する (失敗段階を分類に使うため)。STARTTLS 必須の既定で、サーバーが STARTTLS を提供しなければ平文へ降格せず `configuration_invalid`。1 回の送信・probe には全体の期限 (10 秒。接続から DATA 受理/NOOP まで) を掛ける。QUIT は期限とは別に短く (2 秒) 待ち、DATA 受理後に QUIT が無応答でも確定済みの結果 (送信成功) を覆さない
(lettre の timeout は無応答のサーバーに効かない場面があるため)。宛先は `NormalizedEmail` で検証してから SMTP に渡す (SMTP コマンド注入・複数宛先を許さない)。

## 7. 管理者 API

すべて `instance:administrate` (E15a と同じ判定) のみ。非管理者は 401/403。応答に secret・token・proof・登録者メールは含まれない。

| API | 内容 |
|---|---|
| `GET /api/v1/admin/registration-gate/protections` | 保護ごとの provider・enabled・generation・fingerprint・status・reason・checked_at・valid_until と、最後の検証 (business 結果・reason・transport 分類・最後の成功・generation 一致) |
| `POST /api/v1/admin/registration-gate/protections/{protection}/verify` | 実検証の即時実行 (bootstrap・再検証)。`protection` は `email` / `bot`。body は §3 のとおり。business 結果と transport 分類の両方を返し記録する。`attempt_rejected` は記録しない |

運用手順の例: サーバー設定を投入して再起動 → `GET .../protections` で generation と `unverified` を確認 → bot は widget の token で `verify` → email は `verify {}` → canary の code で `verify {challenge_id, code}` →
両方 `operational` を確認 → gate を開く (`/registration-gate/open`)。設定を変更したら generation が変わり `unverified` に戻るので再検証する。

## 8. E15d への引き渡し

- 観測入力の反映: `RegistrationProtectionReadinessUseCase::sync_registration_observation(_at)` (`RecordRegistrationObservationUseCase` 経由のみ)。probe: `run_registration_protection_probe(_at)`。時刻を引数にした評価: `registration_protection_readiness_at` / `registration_observation_at`。
- hook から: `verify_registration_email_proof` (proof の照合・消費)、`verify_registration_bot_token`。いずれも内部 use case で HTTP の書込口ではない。
- E15d が実装するもの: 認証付き pre-persist hook、継続 probe の周期実行 (`probe_max_age` より短く)、hook 時の gate 最新評価、全 method/全到達経路、設定喪失/latch の統合試験、config attestation。

## 9. 試験

- kernel: 評価・分類・TTL・期限境界・rate limit・proof の判定 (純粋)。
- driver (実 DB): readiness 記録・並行書込、challenge/proof の原子性 (並行確認・並行消費・並行発行)、期限境界、rate limit、E15a 観測入力の反復で revision・通知が増えないこと。
  adapter: 設定読込・generation、Turnstile の分類 (実応答 fixture)・wiremock 経由の実 HTTP (timeout・429/5xx・redirect・本文上限)、SMTP のローカル模擬サーバー経由の実送信
  (認証失敗・宛先拒否・一時障害・接続失敗・timeout・STARTTLS 必須)。
- application (実 DB・fake provider): bootstrap・canary roundtrip・鮮度 (注入時刻)・設定変更・復旧・一方故障・challenge→proof→照合の end-to-end。
- server (実 DB): 認証・認可・入力検証・応答の秘匿・proof 照合が HTTP に出ないこと・OpenAPI。
- 実行: `DATABASE_URL=... cargo test --workspace`。実 Turnstile (公式テストキー): `cargo test -p driver live_turnstile -- --ignored`。

## 10. 未検証・制約

- 実 SMTP サーバーとの TLS handshake・実メール配送・canary mailbox での受信、本番 Turnstile 資格情報と widget 発行の token は未検証 (資格情報を取得していない)。
- Kratos の registration flow 実在確認 (`GET /self-service/registration/flows`) は、Kratos v1.3.1 のソースで挙動 (browser flow は CSRF cookie なしで 403) を確認し、wiremock の契約テスト (200/403/404/410/5xx/timeout) で固定しただけで、実 Kratos との結合は E15d。
- 送信元単位の rate limit は接続元アドレスに基づく (proxy 配下の扱いは §4)。**proxy 配下では上限 (既定 20 通/時間) が全利用者の共有枠になる**ため、信頼済み proxy/edge 側での利用者別の制限は E15d/deployment で用意し試験すること (引き継ぎ事項)。
- **公開応答を観測の判別器にしない設計**: 公開発行要求で宛先を提示した後 (RCPT コマンド送信以降) の失敗は、応答コードや段階だけでは宛先非依存と保証できないため、**すべて当該 attempt の失敗にとどめ** (challenge の内部失効のみ)、readiness・観測を更新しない。公開要求経路から readiness を更新するのは宛先提示前の段階 (接続・STARTTLS・認証・MAIL FROM) の失敗だけで、公開要求の連続失敗を readiness に積み上げることもしない。provider の一時障害/設定不正は **宛先を伴わない独立 probe (`run_registration_protection_probe`) と管理者の canary (固定の canary 宛先。RCPT・DATA・timeout を含む配送失敗を provider の障害として記録する。operational なメールが canary の DATA 452 などで非健全へ遷移することを実 DB で試験)** で検出する (運用では probe を `probe_max_age` より短い周期で回す。周期実行は E15d)。これにより、宛先 A への発行の後に別宛先 B へ発行しても、B の応答 (202/503) から A の受理/RCPT 550/450/421/DATA 失敗/timeout を区別できない (実際の SMTP 分類関数で作った結果による応答列テストと、mock SMTP の実送信で固定)。
- 運用影響: provider が `temporarily_unavailable`/`configuration_invalid` になると、BOT が健全ならメール保護は一時的に必須から外れ (E15a の「健全な対策のみ必須」)、メール proof なしで BOT のみで登録が受け付けられる。メール障害の通知 (`protection_degraded` 等) を運用で確認すること。
