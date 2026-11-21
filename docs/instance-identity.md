# instance identity と instance 署名鍵 (P1 post-relay, K1)

Emumet が ShuttlePub の home へ転送するリクエストの署名者は、Profile ではなく **instance actor** です。この文書は K1 (`emumet-instance-identity`, Emumet#116) が導入する identity・鍵・公開文書の仕様と運用です。転送契約そのもの (envelope・外側署名・受け手の検証手順) の正本は host の `intents/shuttlepub/contracts/emumet-post-relay/` です (C0)。

## 1. 公開される actor

- `GET {PUBLIC_BASE_URL}/ap/instance` (匿名)。`Content-Type: application/activity+json`。
- `id` は `{PUBLIC_BASE_URL}/ap/instance`、`type` は `Application`。Profile actor (`/ap/profiles/{nanoid}`) ではなく、Profile の鍵とも別物です。
- `publicKey` は**配列**です。各要素は `id` (key id URI)・`owner` (= instance actor の id)・`publicKeyPem` を持ちます。複数の公開鍵を同時に表現できます (ローテーション用)。公開されるのは**失効していない鍵すべて** (署名に使う active 1 本と、署名には使わないが公開を続ける retired の鍵) です。K1 が作るのは active な 1 本だけなので、K1 単体では 1 本だけが公開されます。
- key id URI は `{PUBLIC_BASE_URL}/ap/instance#key-{opaque-id}`。`opaque-id` は鍵ごとに 1 回だけ生成するランダムな 32 桁 16 進で、**再利用しません** (DB の主キー・`key_id_uri` の UNIQUE、行は削除不可)。
- すべての URL は設定 (`PUBLIC_BASE_URL`) から生成します。要求の `Host` ヘッダは使いません。
- 秘密鍵・暗号化済み秘密鍵・KDF の salt などは公開しません。公開するのは公開鍵 PEM だけです。
- 保存済みの identity が設定と食い違う、または鍵が壊れている場合、この route は 500 を返します (誤った URL の文書を返しません)。

## 2. 保存

migration `20261122000100_instance_identity.sql` (追加のみ。既存テーブルは変更しません)。

| テーブル | 内容 |
|---|---|
| `instance_identity` | deployment に 1 行 (`singleton` が主キーかつ `CHECK (singleton)`)。作成時の `public_origin` を保持 |
| `instance_signing_keys` | 鍵。`id` (opaque)・`algorithm`・`encrypted_private_key` (JSONB)・`public_key_pem`・`key_id_uri` (UNIQUE)・`revision`・`retired_at`・`revoked_at` |

- **署名に使う状態と公開する状態は別です。** 鍵は次のどれかです。

  | 状態 | 条件 | 署名 | 公開 |
  |---|---|---|---|
  | active | `retired_at` も `revoked_at` も NULL | する | する |
  | retired | `retired_at` あり、`revoked_at` NULL | しない | する (過去の署名を検証できるように) |
  | revoked | `revoked_at` あり | しない | しない |

- **active 鍵は 1 本**: `retired_at IS NULL AND revoked_at IS NULL` の部分 UNIQUE index `instance_signing_keys_one_active`。retired・revoked の鍵の数に制限はありません。active から retired / revoked へ動かす操作は K2 で、K1 にはありません。
- identity と鍵の素材・識別子は不変です。`instance_identity` の UPDATE / DELETE / TRUNCATE と、`instance_signing_keys` の DELETE / TRUNCATE、および `retired_at`・`revoked_at`・`revision` 以外の列の UPDATE は trigger が拒否します。黙った再生成・差し替えを DB 側でも防ぎます。
- 秘密鍵は Profile 鍵と同じ既存の RSA-2048 生成・Argon2 + AES-GCM による暗号化を再利用し、同じ master key password (`/run/secrets/master-key-password`、開発では `./master-key-password`) で保護します。保存先は別テーブルです。
- 旧バイナリはこの migration を知らないため、適用後は起動に失敗します。戻す場合は forward fix とし、テーブルを DROP しません (使い捨て DB の複製で確認してください)。

## 3. 起動時の provisioning

`server` は migration と Profile 鍵の provisioning の後、worker・listener より前に `EnsureInstanceIdentityUseCase` を実行します。

1. 1 transaction の中で advisory lock (`pg_advisory_xact_lock`) を取り、**identity も鍵も無いときだけ**生成します。複数プロセスが同時に起動しても identity・active 鍵は一組です。ロックに加えて上記の DB 制約が最後の砦で、衝突した側は `Concurrency` エラーになり何も置き換えません。
2. 既にある場合は、次をすべて検査し、1 つでも外れれば**起動を止めます** (何も書き換えません)。
   - 保存済み origin と設定 `PUBLIC_BASE_URL` (正規化後) の一致 (`OriginMismatch`)
   - 鍵 id URI が origin から導いた値と一致 (`KeyIdUriMismatch`)
   - active 鍵がちょうど 1 本 (`NoActiveKey` / `MultipleActiveKeys`。retired・revoked の鍵だけの identity も `NoActiveKey`)、アルゴリズムが RSA-2048 (`UnsupportedAlgorithm`)
   - 秘密鍵が現在の master key password で復号でき (`UndecryptableKey`)、署名用の鍵として parse でき (`UnparsableKey`)、保存済み公開鍵と対になっていること (`KeyPairMismatch`)
   - identity なしで鍵だけがある状態 (`KeysWithoutIdentity`)
3. 復号不能・破損・origin 不一致は「未生成」として扱いません。新しい鍵を作って隠すことはしません。

`PUBLIC_BASE_URL` を変えると、既存の instance identity は起動時に拒否されます。origin の変更は K2 以降の運営判断 (identity の移行) であり、K1 に操作はありません。`https://Emumet.Example:443/` のように、正規化すると同じ origin になる表記違いは不一致ではありません。

master key password が読めない場合も起動は失敗します (instance 鍵を unlock できないため)。

## 4. 秘密鍵のメモリ保持

起動時の unlock で、秘密鍵を復号して**一度だけ parse** し、parse 済みの署名ハンドル (`PreparedSigner`) を `UnlockedInstanceSigningKey` に持たせてプロセス内の `InstanceKeyCache` に保持します。PEM そのものは保持しません。署名は `UnlockedInstanceSigningKey::sign` で行い、署名ごとの KDF も PEM の再 parse も走りません。Profile 鍵の署名経路 (`Signer`、署名ごとに parse) は変更していません。

- cache key は key id + `revision`。上限 2 本、TTL 15 分。
- miss は single-flight (並行する呼び出しは 1 回の KDF の結果を共有)。要求ごとの KDF は走りません。
- TTL 経過後は DB から再 unlock します。そのとき復号に失敗すれば**エラー**で、古い cache を使い続けません。
- parse 済みの鍵は drop 時にゼロ化され (復号した PEM は parse 直後に drop)、`Debug` は `[REDACTED]`。ログ・DTO・ディスクには出しません。
- TTL は、DB 上で失効した鍵が cache に残る時間の上限にすぎず、**緊急失効の保証ではありません**。通知・操作・失効時の cache 無効化は K2 の範囲です。`revision` が、その無効化のための境界です。

delivery worker (S1) は `UnlockInstanceSigningKeyUseCase` から鍵を得て署名します。

## 5. K1 の範囲外 (K2)

次の操作は追加していません (repository にも、HTTP にも、運営者向けサブコマンドにもありません)。

- 鍵の rotate / revoke、旧鍵の公開猶予、緊急失効
- identity の差し替え・origin の移行

`instance_signing_keys.retired_at` / `revoked_at` / `revision`、公開文書が複数の鍵を扱えること、鍵ハンドル (key id + revision) の境界は、K2 がこれらを実装するための土台です。

## 6. 検証 (CI)

- `application/src/instance_identity/tests.rs`: 並行 provisioning、再起動で不変、秘密情報の非公開、keyId 非再利用・active 1 本のまま複数の公開鍵 (retired の鍵の公開、revoked の鍵の非公開)、parse 済みハンドルの再利用 (KDF・parse が 1 回)、復号不能・破損・origin 不一致で再生成しない、cache (single-flight・TTL・上限)、現 main 相当 DB への migration。
- `server/src/route/activitypub/instance_actor_tests.rs`: `/ap/instance` が本番 root router で匿名・`Application`、`Host` 偽装に影響されない、Profile actor の回帰なし。
- DB を使う試験は `DATABASE_URL` が必要です。未設定のときは skip されます。skip は合格ではありません (CI では DB が用意されます)。
