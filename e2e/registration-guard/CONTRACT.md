# E15c/E15d への観測・配線契約

実験から導く設計契約であり、production adapter/router の実装ではない。
メール方式の最終選択は host ADR に従う。E15b 完了は D15 全体の受け入れではない。

## 前提と選択肢

| 保護 | Kratos native | Emumet webhook | 判断 |
|---|---|---|---|
| 登録ゲート | 静的 registration.enabled だけでは latch/override/最新状態を持たない | E15a 状態と新鮮な provider 観測を submit 時に評価 | Emumet |
| BOT token | v1.3.1 schema に native captcha method なし | transient_payload を Siteverify へ送る | Emumet、Turnstile を基準 provider |
| メール所有 | password verification は persist 後。passwordless code は persist 前に verified address を作れる | flow/email に束縛した事前 email proof なら password 登録を維持 | **Emumet 登録前 proof を採用** |

準備条件は **メール OR BOT**。両方健全なら両方必須、片方障害なら健全側のみ必須。
両方一時障害は一時拒否/自動復旧、設定喪失は停止 latch/手動再開、手動閉鎖は自動解除しない。
override は管理者の永続状態だけから読み、**override 中も effective な保護を課す**。
client の `override=true` は権限でも proof でもない。fixture の override は hook 許可相当だけを測る。

## メール方式の決定と proof 契約

ユーザー決定 `question-88b36432ff84f589f1388699c8f40fa3` は **A: Emumet 登録前メール確認、password 登録を維持**。
B（native code 登録）は実証上可能だが、新規登録を passwordless に変え、登録後 password 設定と
メール障害時の password fallback を要するため採用しない。Kratos post-registration verification は所有確認の代用にしない。

E15c は SMTP による challenge 発行/確認と opaque proof を実装し、E15d が hook へ接続する。

1. 正規化 email + 実 registration flow ID + config generation に challenge を束縛。メール送信成功だけで proof を発行しない。
2. code/link の確認で所有を検証後、推測困難な proof を発行。DB は token/code の hash、期限、試行回数、消費先を保持。
   有限 TTL かつ flow の期限以内にし、具体 TTL・試行/送信 rate limit は E15c で固定/境界テストする。
3. client は `transient_payload.email_proof` に proof を渡す。hook が実 context の email/flow と照合し、期限切れ・改変・別 email/flow・旧 generation を拒否。
4. 消費は原子的かつ一回性。同じ flow の Kratos transport retry にだけ同じ検証済み事実を返し、別 flow への replay は拒否。gate は毎回最新評価。
5. proof の健全性観測と、登録者ごとの所有証明を分離。probe/canary 成功は別の登録者の proof にならない。

SMTP は既存運用可能な transport を利用する抽象で、特定の有料事業者を契約しない。
fixture はローカル sink。本番資格情報の取得/メール送信は未実施。
SMTP 認証不正/空 URL/TLS設定不正は configuration_invalid、接続/timeout/4xx は temporary、
宛先拒否（例550）は当該 attempt の失敗として区別する。HTTP provider の429/5xxと SMTP全5xxを同一扱いしない。
Emumet が実際に使う SMTP config generation と送信/確認結果を観測し、Kratos courier 設定の存在から ready と推論しない。

## Kratos 配線（E15d 所有）

`selfservice.flows.registration.after.password.hooks` に次を置く。
native code を有効にする場合は `after.code.hooks` にも同じ hook を置く。
**有効な全 registration method を列挙し、未保護 method は無効化する。**
global の `registration.after.hooks` だけでは pre-persist を保護しない。

```json
{
  "hook": "web_hook",
  "config": {
    "url": "https://private-emumet/registration-guard",
    "method": "POST",
    "body": "file:///immutable/registration-hook.jsonnet",
    "response": {"parse": true, "ignore": false},
    "emit_analytics_event": false
  }
}
```

- pre-persist は **method-specific + parse:true**。parse:false は post-persist となり、拒否でも identity が残る。
  deprecated `can_interrupt:true` を代用しない。
- hook は private/authenticated 経路、TLS・到達元制限/認証を必須にする。
  fixture の unauthenticated HTTP/control endpoint を本番へ持ち込まない。
- context は `identity.traits.email`、`identity.verifiable_addresses`、
  `flow.id/type/transient_payload` の allowlist。credential/cookie/session は送らない。
  native code の verified address も**認証済み Kratos hook context のみ**信頼する。
- candidate identity ID は zero UUID になり得る。重複排除キーにしない。
- browser JSON/form、API、閉鎖前 flow の最終 submit が同じ hook を通る。
  UI/BFF 制限を境界にしない。OIDC/passkey/WebAuthn 等の将来有効化には同じ拒否 matrix が必要。
- login/settings/recovery、認証済み `POST /api/v1/accounts` に hook を置かない。
  admin identity API は self-service 対象外であり、外部には公開しない。

### hook 応答・再試行

| 応答 | v1.3.1 の意味 | adapter 契約 |
|---|---|---|
| 204 空本文 | 許可して persist | 許可の標準形 |
| 403 + Kratos validation messages | 拒否、identity なし | 閉鎖/設定不足/proof失敗の標準形 |
| 200 JSON | identity/flow mutation を parse。`allowed:false` は無視され許可 | boolean 判定プロトコルにしない |
| 200 不正 JSON | parse:true なら拒否 | エラーを200で返さない |
| 201 不正 JSON | parse されず許可 | 使わない |
| 429/503/不達 | retry 後に拒否 | 内部失敗分類をそのまま hook HTTP に流さない |
| timeout | 約30秒x3+backoff、約93秒後 identity なし。public は EOF にもなる | provider deadline は30秒より十分短く（例3秒、hook全体5秒以内） |

retry は3呼出を観測。proof 検証/消費は flow ID + email + token/proof hash + config generation に束縛し冪等化する。
同一 flow retry は同じ proof の検証済み事実を再利用できるが、**gate 許可 verdict は再利用せず毎回最新評価**。
別 flow/別 email への replay は拒否する。
閉鎖と Kratos persist を二つの DB 跨ぎで原子的にすることは未検証であり、
「hook 評価完了後に始まった閉鎖まで全件取り消す」とは主張しない。

## 実効設定・readiness

Kratos admin `GET /health/config` は merged 実効 config の SHA256 を返す（fields は返さない）。
`/admin/config` は404。`health/ready` は hook 消失時も200だった。
ファイルの存在・env 宣言・health 200・hash の存在だけでは ready にならない。

必要な観測レコード（E15c で具体 API/schema を固定）:

- provider/type、enabled、config generation、非 secret な fingerprint
- status（configuration_invalid / operational / temporarily_unavailable / unverified）と reason
- checked_at / valid_until。generation 不一致・期限切れ・probe 停止は旧成功を破棄
- actual verification API の business success、HTTP分類、期待 hostname/action 等の照合
- 各 Kratos replica の binary/image digest・実効 config hash と trusted deployment manifest の照合
- template/schema 等の参照ファイルは内容 digest も別途照合（config hash だけで覆えると仮定しない）
- 同じ artifact/config の隔離 canary で positive 許可と negative 拒否を測り、admin/DB identity 差分を確認

**hook 自体の削除は guard が検知/拒否できない**。実験でも identity が作成された。
監視/poll だけでは消失直後の bypass window を消せない。
E15d/deployment は immutable config と起動時 attestation、未照合/不明 replica を公開しない ingress/admission を用意し、
Kratos public の全経路（直接 API を含む）に迂回 route がないことを保証する。
provider 設定だけの喪失（hook は残る）と enforcement 自体の喪失は別障害である。
本 fixture は本番 ingress の保証までは検証していない。

## provider と失敗分類

BOT の基準は Cloudflare Turnstile `POST https://challenges.cloudflare.com/turnstile/v0/siteverify`。
公式テストキー結果は protocol 検証であり、本番 credential の ready 証明ではない。
HTTP200でも `success:false` は失敗。成功時も本番は expected hostname/action、TTL、一回性を照合する。
空 response/dummy token だけでは実 secret の動作確認にならない。
正しい sitekey 由来の有効 token による business success を当該 generation に束縛する。
未検証は ready とせず、検証/登録の bootstrap も E15c で設計する。

| 条件 | 分類 | ゲートへの意味 |
|---|---|---|
| 未設定/空設定/明示無効 | configuration_invalid | 両対策なしなら停止 latch |
| invalid/missing secret、SMTP 認証/設定不正 | configuration_invalid | 再設定。旧成功は使わない |
| token 空/invalid/expired/duplicate、hostname/action不一致、email proof不一致 | attempt_rejected | 当該登録のみ拒否。不正 token で provider 全体を障害扱いしない |
| timeout/DNS/connect/TLS/429/5xx、不正な provider 応答 | temporarily_unavailable | 健全側へ縮退。両方なら一時拒否、復旧で自動再開 |
| business success + binding/freshness 合致 | operational | effective。health/HTTP200とは別 |
| 設定変更後/成功期限切れ/probe 停止 | unverified | 古い ready の無期限 cache は不可 |

実際の Turnstile 応答は `evidence/provider.json`。
429/5xx/timeout は live provider を故意に障害化せず hook fault injection で fail-closed を測った。
provider adapter の分類テスト/期限/上限は E15c の必須受け入れである。
secret/token/proof本文/登録者メールを readiness API・通常ログ・外部通知へ出さない。

メール transport は SMTP（実験はローカル sink）。SMTP250/接続成功はメール所有確認ではない。
選択方式の実 code/proof 検証成功を address/flow/期限に束縛する。
本番配送 probe は承認済み canary mailbox roundtrip と設定 generation を観測し、
一回の配送成功から全宛先の到達保証はしない。実配送サービスの契約/資格情報取得は本 unit に含まない。
