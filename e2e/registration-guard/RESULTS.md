# 実証結果 — E15b

## 実行条件と結論

2026-11-10、**全 suite exit 0**。registration/regression **29ケース** + captcha schema拒否、
別プロセスの公式 Turnstile **6条件**が成功。helper/証拠整合性 unit test は6 passed。
Kratos **v1.3.1 / build 36e624c716e45ab405599c71b42c384dd45bdfbb** の実バイナリ、
PostgreSQL 16、Podman 6.1.4 / podman-compose 1.6.0、upstream Rust1.97.0 の実 Emumet を使用。
Emumet source base は `0bfa4372e7585b04f9671a70e00fe3ddc30545f7`。
image digest・ソース/証拠 SHA256 は [manifest](evidence/manifest.json)、binary 出力は [version](evidence/version.json)。

**主前提は成立**: method-specific `after.password.hooks` + `response.parse:true/ignore:false` で、
拒否した browser/API の identity は admin/DB の両方で作成されない。
global hook、parse:false、health-only readiness は同等ではない（下記の負の対照）。
本番への接続、実 provider credential、Emumet 事前 email proof の実装は行っていない。

## 条件・期待・観測・証拠

新identity は各ケースの前後差分。既存identityを含む全UUID集合を admin/DB で突合し、
拒否後は1秒後にも再観測する。数値0は空DBを意味するのではなく「新規作成0」を意味する。
「負の対照」は採用不可の配線/応答を意図的に測る実験で、productionの許容動作ではない。

| 条件 | 期待 | 観測 | 証拠 |
|---|---|---|---|
| API直接submit、閉鎖 | 拒否/新identity 0 | 400; 新identity 0; hook 1 | [deny_api](evidence/deny_api.json) |
| browser JSON直接submit、閉鎖 | 拒否/0 | 400; 新identity 0; hook 1 | [deny_browser](evidence/deny_browser.json) |
| cookie/CSRF + 通常form直接submit | 拒否/0 | 400; 新identity 0; hook 1 | [deny_browser_form](evidence/deny_browser_form.json) |
| 開放時API flowを閉鎖後submit | 拒否/0 | 400; 新identity 0; hook 1 | [old_flow_api](evidence/old_flow_api.json) |
| 開放時browser flowを閉鎖後submit | 拒否/0 | 400; 新identity 0; hook 1 | [old_flow_browser](evidence/old_flow_browser.json) |
| server override相当の許可/API | 許可/1 | 200; 新identity 1; hook 1 | [override_api](evidence/override_api.json) |
| server override相当の許可/browser | 許可/1 | 200; 新identity 1; hook 1 | [override_browser](evidence/override_browser.json) |
| hook HTTP503 | retry後拒否/0 | 400; 新identity 0; hook 3 | [server_error_api](evidence/server_error_api.json) |
| hook HTTP429 | retry後拒否/0 | 400; 新identity 0; hook 3 | [rate_limit_api](evidence/rate_limit_api.json) |
| hook各40秒sleep、Kratos deadline超過 | fail-closed/0 | public EOF; 新identity 0; hook 3 | [timeout_api](evidence/timeout_api.json) |
| hook TCP切断 | fail-closed/0 | 400; 新identity 0; hook 3 | [disconnect_api](evidence/disconnect_api.json) |
| hook port不達 | fail-closed/0 | 400; 新identity 0; hook 0 | [unreachable_api](evidence/unreachable_api.json) |
| parse:false + can_interrupt:true、不正200 | 負の対照:作成される | 200; 新identity 1; hook 1 | [malformed_unparsed](evidence/malformed_unparsed.json) |
| parse:false、204 | 負の対照:作成される | 200; 新identity 1; hook 1 | [empty_unparsed](evidence/empty_unparsed.json) |
| parse:true、不正200 | 拒否/0 | 400; 新identity 0; hook 1 | [malformed_parsed](evidence/malformed_parsed.json) |
| parse:true、204 | 許可/1 | 200; 新identity 1; hook 1 | [empty_parsed](evidence/empty_parsed.json) |
| parse:true、200 allowed:false | 負の対照:作成される | 200; 新identity 1; hook 1 | [false_json_is_not_denial](evidence/false_json_is_not_denial.json) |
| parse:true、不正201 | 負の対照:作成される | 200; 新identity 1; hook 1 | [created_malformed_is_not_parsed](evidence/created_malformed_is_not_parsed.json) |
| 既存identity、gate閉鎖でlogin | 成功、hook 0 | 200; identity不変; hook 0 | [login](evidence/login.json) |
| parse:falseのpost-persist hook拒否 | 負の対照:エラーでもidentity残存 | 502; 新identity 1; hook 1 | [post_persist_deny](evidence/post_persist_deny.json) |
| global after.hooksのみparse:true | 負の対照:hook未呼出で作成 | 200; 新identity 1; hook 0 | [global_hook_not_pre_persist](evidence/global_hook_not_pre_persist.json) |
| method hookを設定から削除 | 負の対照:health正常でも作成 | 200; 新identity 1; hook 0 | [hook_removed_canary](evidence/hook_removed_canary.json) |
| native verification無効、guard維持 | config hash変化、health正常、拒否/0 | 400; 新identity 0; hook 1 | [verification_removed_guard_remains](evidence/verification_removed_guard_remains.json) |
| client transient_payloadにfake override | 拒否/0 | 400; 新identity 0; hook 1 | [untrusted_override_payload](evidence/untrusted_override_payload.json) |
| 未有効化OIDC method直接submit | 拒否/0、hook不要 | 404; 新identity 0; hook 0 | [disabled_oidc](evidence/disabled_oidc.json) |
| passwordless:falseのcode直接submit | 拒否/0、hook不要 | 400; 新identity 0; hook 0 | [disabled_code](evidence/disabled_code.json) |
| 正しいnative code、verified=trueの後guard拒否 | 拒否/0 | 400; 新identity 0; verified=true観測 | [native_code_gate_denied](evidence/native_code.json) |
| 正しいnative code、guard許可 | 作成/verified=true | 200; 新identity 1; verified=true観測 | [native_code_allowed](evidence/native_code_allowed.json) |
| 同じ有効JWTでAccount追加、gate閉鎖/故障 | 201/201、無効JWT401、hook/identity増分0 | 201 / 201 / 401; hook 0 | [account](evidence/account.json) |


### メール / context / 実効設定

- [password許可](evidence/override_api.json): native verification enabled でも登録時の住所は **verified:false**、identityは既にpersist。
- [native code開始](evidence/native_code_initiated.json): local SMTPへcodeを発行しても identity 未作成。
  [正code後の拒否](evidence/native_code.json): hook candidateのaddressは **verified:true**、拒否で未作成。
  [許可](evidence/native_code_allowed.json): verified:true identityを作成。
- [transient_payload](evidence/override_api.json): flow内に BOT token/email proofの合成値が届く。
  これはproofのセキュリティ実装ではなく、tokenを渡せることの観測。
- [設定API](evidence/config_endpoints.json): `/health/config`=200 SHA256、`/admin/config`=404、health=200、version=v1.3.1。
- [hook削除](evidence/config_loss.json): hash変化、health=200、guard未呼出でidentity作成。
  設定ファイル/health正常だけではreadyを証明できない。
- [verification無効化](evidence/verification_removed_guard_remains.json): hash変化でもguardが残れば拒否できる。
- [captcha schema](evidence/unsupported_captcha.json): 実バイナリが additionalProperties "captcha" not allowed、exit1。
- [隔離](evidence/isolation.json): 全runtime containerで volume-type mount 0、image既定volumeもtmpfs。

### timeout の意味

[timeout](evidence/timeout_api.json) は **93.174秒 / 3 hook calls / public RemoteDisconnected**。
[Kratos server側](evidence/timeout_server.json) は内部HTTP504を記録するが、Go HTTP write deadline超過のため
clientが504本文を受け取れるとは限らない。HTTP statusだけで成功/失敗を推定せず、admin/DB identity差分を確認した。

### Turnstile の実API（公式公開テストキーのみ）

全レスポンス: [provider.json](evidence/provider.json)。本番キーのready証明ではない。

| 条件 | HTTP | success / error-codes |
|---|---:|---|
| 公式pass secret + dummy token | 200 | true |
| 公式reject secret | 200 | false / invalid-input-response |
| 公式duplicate secret | 200 | false / timeout-or-duplicate |
| secret空 | 400 | false / missing-input-secret |
| 無効な合成secret | 400 | false / invalid-input-secret |
| response空 | 200 | false / missing-input-response |

429/5xx/timeout の実 provider 障害は誘発していない。hook境界の故障注入と、E15c の provider adapter 分類要件を区別する。

## 保護の所在（ユーザー決定）

`question-88b36432ff84f589f1388699c8f40fa3` の回答 **A**:
メール所有確認は **Emumet登録前email proof + SMTP**（password登録を維持）。
BOT検証は **Emumet webhook + Turnstile Siteverify**、gate判定も **Emumet webhook**。
Kratos native codeは技術的に可能だが、passwordlessへの方式変更を要するため採用しない。
native post-registration verificationは登録前所有確認の代用にならない。
詳細・却下案・proofの期限/一回性/flow-email束縛・観測/失敗分類は [CONTRACT.md](CONTRACT.md) と
[host ADR](https://github.com/ShuttlePub/host/blob/main/intents/emumet/decisions/registration-protection-placement.md)。

## 再現と未検証事項

[README](README.md) のNix/native手順に `--fresh-databases --skip-provider` を付けて全ケースを一括実行し、
続けてdevshell外で `provider_probe.py` を実行した（既存DBを削除せず一意な新DBを作成）。
中断調査時のPodman mount/TTY、app log/CA/Keto/JWKS cache/一意名の問題は修正後に全suiteを再実行済み。
Nix devshell内のPython HTTPS support不整合は外部probe分離で回避する。

- portable Dockerfile buildは未実行。実証済みnative経路と区別する。
- 本番 credential/実メール配送、production ingress・hook認証、設定喪失時の全replica排除はE15c/E15d。
- overrideの全D15状態、両方/片方障害時の保護強制、email proofの攻撃/期限/replay試験は後続。
- hook自体を消すと無保護になる。immutable config・未attest replica非公開をdeployment側で保証し、UIの表示制限だけで保護しない。
- hook評価後〜persist間の管理者閉鎖を二つのDB跨ぎで原子的にすることは未検証。
- teardownはレビューtimeoutにより未実行。専用tmpfs stackと旧実験volumeはoperator手動cleanup依頼として明示した。

## 参照した公式仕様・ソース

- [v1.3.1 schema](https://github.com/ory/kratos/blob/36e624c716e45ab405599c71b42c384dd45bdfbb/.schemastore/config.schema.json)
- [web_hook / parse response](https://github.com/ory/kratos/blob/36e624c716e45ab405599c71b42c384dd45bdfbb/selfservice/hook/web_hook.go)
- [method-specific hook registry](https://github.com/ory/kratos/blob/36e624c716e45ab405599c71b42c384dd45bdfbb/driver/registry_default_registration.go)
- [effective config hash handler](https://github.com/ory/kratos/blob/36e624c716e45ab405599c71b42c384dd45bdfbb/driver/config/handler.go)
- [native code registration](https://github.com/ory/kratos/blob/36e624c716e45ab405599c71b42c384dd45bdfbb/selfservice/strategy/code/strategy_registration.go)
- [Turnstile testing](https://developers.cloudflare.com/turnstile/troubleshooting/testing/) / [Siteverify](https://developers.cloudflare.com/turnstile/get-started/server-side-validation/)
