# Profile 連合への in-place 切替 (P4 `profile-federation-in-place-cutover`)

旧 Account 単位の連合 (actor `/ap/accounts/{id}`・Account 単位の鍵/Follow/block/mute) を、Profile 単位の連合
(actor `{PUBLIC_BASE_URL}/ap/profiles/{profile_nanoid}`) へ**同じ DB のまま**切り替える手順。方針は host ADR
`profile-federation-in-place-reset` (旧連合データだけを初期化。変換・コピー・dual-write・Move・無停止移行はしない)。

> **これは不可逆な破壊的操作を含む。実行前に必ず operator の確認を得ること。**
> 切替後に旧 Account 単位モデルへ戻す逆変換は保証しない。戻す必要があるときは、切替前に取ったバックアップから復元する。

## 1. 何が起きるか

新バイナリの起動 (`AppModule::new` が `migrations/` を自動適用する) が切替のトリガになる。

| 順 | 処理 | 担当 | 失敗時 |
|---|---|---|---|
| 1 | `20261120000001_profile_social_graph.sql`: Profile 単位の関係・配送テーブル (`profile_follows` `profile_blocks` `profile_mutes` `profile_inbound_activities` `profile_delivery_jobs` `profile_delivery_attempts`) を追加 (非破壊) | migration | 起動失敗。DB は変わらない (transaction) |
| 2 | `20261120000002_reset_legacy_federation.sql`: **前提検査** (username 未設定の Profile が 1 件でもあれば `RAISE EXCEPTION`) → 旧連合データの `TRUNCATE` → 台帳 `profile_federation_resets` に `legacy-account-federation-v1` を記録。全て 1 transaction | migration | 起動失敗。**何も消えず台帳も残らない** (切替拒否) |
| 2b | `20261121000001_profile_signing_keys_survive_deactivation.sql`: `profile_signing_keys.profile_id` の外部キー (ON DELETE CASCADE) を外す (非破壊)。Account 無効化の projection が `profiles` 行を消しても Profile 鍵は保持され、再有効化で行が再 materialize されると同じ鍵で復帰する (下記「Account の状態と Profile」) | migration | 起動失敗。DB は変わらない |
| 3 | Profile 鍵の一回限り生成 (`server/src/cutover.rs` → `EnsureProfileSigningKeysUseCase`): **有効な鍵を持たない Profile にだけ**鍵を作る。1 transaction・advisory lock で直列化 | 起動処理 (`main`、root router / worker より前) | 起動失敗 (鍵のない Profile を actor として公開しない) |
| 4 | projection worker・webhook worker・**Profile 配送 worker** を起動し、root router を serve | `main` | — |

### Account の状態と Profile (切替後の挙動)

- **停止 (suspend)・BAN 中**の Account の Profile は actor ではない: actor 文書・WebFinger は 404、inbox は 404 (相手には「存在しない」と見える)、その Profile からの新規 Follow は 403、停止中の Profile を対象にした Follow は 400。**Profile 行・関係・配送 job は残る**: 配送 worker は `claim_due` でその Profile の job を取得しないだけで、キューに待機し (試行回数も消費しない)、解除すると送信される。ルールは `AccountQuery::find_by_id` と同じ (期限付き停止は期限が切れれば自動で解除)。
- **無効化 (deactivate) 中**は上記に加えて **Profile 行そのものが projection から消える**。`profile_follows` `profile_blocks` `profile_mutes` `profile_delivery_jobs` などは Profile 行への外部キー (ON DELETE CASCADE) で**一緒に消える** (待機ではなく消去)。再有効化で Profile 行は戻るが、消えた関係・未送信の job は戻らない。保持されるのは Profile の identity (id・nanoid・username) と鍵 (`profile_signing_keys` は外部キーを持たない) だけ。
- 停止・BAN の解除 (unsuspend / unban / 期限切れ) で、**同じ actor URL・同じ鍵**のまま復帰し、待機していた job が送られる。再有効化でも同じ actor URL・鍵で復帰するが、上記のとおり消えた関係・job は戻らない。
- **無効化→再有効化**: 無効化で Account の projection が `profiles` 行を削除する。再有効化の projection (Account projector) が、その Account が所有する Profile を**全 event 履歴から再 materialize** する (tailing の窓 (直近 100 件) に依存しない)。Profile の id・nanoid・username と鍵は変わらない。
- Profile 鍵は `profiles` 行の有無と独立に保持される。将来 Profile を恒久削除する経路を作る場合は鍵の**明示的な削除**が必要で、「projection の行がない＝不要な鍵」として掃除してはならない (再有効化で復帰できなくなる)。

### 初期化の対象 (`TRUNCATE TABLE` で明示列挙。`CASCADE` なし)

`follows`、`blocks`、`mutes`、`remote_accounts`、`outbox_activities`、`signing_keys` (旧 Account 連合鍵)

### 保持するもの (一切触れない)

`auth_hosts` `auth_accounts` `auth_emumet_accounts` `accounts` `profiles` `metadatas` `images` `organization_members`
`profile_transfer_requests` (+events) `account_events` `profile_events` `metadata_events` `account_report_events` `account_reports`
`registration_gate_*` `projection_checkpoints` `profile_domain_controls` `profile_signing_keys`、新モデルの全テーブル。
対象を参照する外部キーが非対象テーブルにあると `TRUNCATE` は失敗する (黙って巻き込まない)。旧リモート icon の孤児 `images` は残る (共有資産なので掃除しない)。

### 一回限り・再実行の保証

- 台帳に `legacy-account-federation-v1` があれば、migration を再適用しても (再起動・手動再実行) 再消去しない。初期化**後**に作られたデータは消えない。
- 鍵生成は「有効な鍵のない Profile」だけが対象で、既存の鍵を再生成・上書きしない (`profile_signing_keys` の「Profile あたり有効鍵 1 つ」の部分 unique index が最後の砦)。
  再起動しても鍵は変わらない。切替後に作られる Profile は作成 transaction の中で鍵が作られる (`POST /api/v1/accounts` / `POST /api/v1/profiles`)。
- 旧 `signing_keys` から Profile 鍵への変換はしない。

## 2. 事前条件

1. **全既存 Profile に username が設定済み**であること (FN `profile-identity-web-adaptation` のマージ後、`POST /api/v1/profiles/{profile_nanoid}/username` で設定)。
   未設定が残っていると、**最初の P4 migration (`20261120000001`) の先頭の検査**が `profile federation cutover refused: N profile(s) have no username ...` で abort する (`20261120000002` にも同じ検査があるが、通常は手前で拒否される)。
   migration は P4 の 1 件も適用されず、DB は切替前のまま・旧データも消えない。ただし**新バイナリは migration に失敗して起動しない**ので、復旧は §3.1 の手順で行う。
   事前確認 (psql): `SELECT count(*) FROM profiles WHERE username IS NULL;` が 0。
   DB の `NOT NULL` 化は行わない (旧イベント列からの projection 再構築で username を持たない Created イベントが残りうるため)。作成 API 側で必須化している。
2. master key password (`/run/secrets/master-key-password`、開発では `./master-key-password`) が新バイナリから読めること (鍵の生成・署名に使う)。
3. DB のバックアップを取得済みであること。
4. `PUBLIC_BASE_URL` が運用値であること (actor URL・key id はこの値から作られ、鍵 id の URI に焼き込まれる)。
5. projection が追いついていること (`projection_checkpoints`)。追いついていない Profile は鍵生成の対象に入らないが、次回起動の生成処理が拾う。

## 3. 切替手順

1. **旧機構を停止**する: 旧 inbox・Account 単位の関係書込 (旧 follow/block/mute API)・旧配送 worker を含む**旧プロセス全体**を停止する。新バイナリの起動前に必ず止める (migration 中に旧プロセスが旧テーブルへ書くと検証できない)。
2. バックアップを取り、事前条件を確認する (§2)。
3. **新バイナリを 1 プロセスだけ起動**する。起動ログで次を確認する:
   - migration の適用 (拒否された場合は `profile federation cutover refused` で終了する。username を設定して再起動する。何も変わっていない)
   - `generated the one-time actor keys of Profiles that had none` (created = 既存 Profile 数) または `every Profile already has an actor key`
4. **スモーク確認**:
   - `GET /.well-known/webfinger?resource=acct:{username}@{PUBLIC_BASE_URL の host[:port]}` → 200、`self` link が `{PUBLIC_BASE_URL}/ap/profiles/{nanoid}`
   - `GET /ap/profiles/{nanoid}` (Accept: `application/activity+json`) → 200、`publicKey.id` が `{actor}#main-key`
   - `GET /ap/accounts/{account_nanoid}` → **410 Gone** (新 actor へ redirect しない)、`/internal/v1/accounts/{id}/sign` → 410
   - 外部 instance (Mastodon / Iceshrimp) から Profile actor を検索して Follow → `GET /api/v1/profiles/{nanoid}/followers` に載り、Accept が配送される
   - `POST /api/v1/profiles/{nanoid}/follows` で外部 actor を Follow → 相手の inbox に署名付き Follow が届く
5. 旧バイナリへ戻さない。問題があればバックアップから復元する。

### 3.1 username 検査で拒否された場合の復旧

拒否時点で DB は切替前の状態 (P4 の migration は未適用、旧連合データ・鍵・台帳なし) のままなので、**バックアップ復元は不要**。新サーバーは起動できないため、username の設定は**切替前のバイナリ**で行う。

1. 新バイナリを停止したまま (起動を繰り返さない)、拒否メッセージの件数を確認する: `SELECT id, nanoid FROM profiles WHERE username IS NULL;`
2. **切替前のバイナリ**を、外部からの受信・配送を再開しないよう (ingress を閉じたまま、管理 API だけに到達できる経路で) 起動する。
   `_sqlx_migrations` には切替前の migration しか入っていないので、旧バイナリはそのまま起動できる。
3. 該当 Profile の所有者が `POST /api/v1/profiles/{profile_nanoid}/username` で username を設定する (一回限りで変更不可。FN の画面経由でもよい)。
4. `SELECT count(*) FROM profiles WHERE username IS NULL;` が 0 になったら旧バイナリを停止し、§3 の手順 3 から再試行する。
5. もし検査後に migration の途中で失敗した場合も、各 migration は 1 transaction なので部分適用は残らない。原因を直して再起動すればよい。それでも戻せない状態になったら (例えば誤って部分的に手で変更した場合) §2.3 のバックアップから復元する。

複数プロセス/レプリカで運用する場合も、台帳・advisory lock・unique index により二重初期化・二重鍵生成は起きない (が、切替は 1 プロセスで行うこと)。

## 4. 廃止した route (最終一覧)

| 旧 route | 切替後 |
|---|---|
| `GET /ap/accounts`、`/ap/accounts/{account_id}` と `/inbox` `/outbox` `/followers` `/following` (全 method) | **410 Gone** (`route::retired`)。新 actor へ redirect しない |
| `GET /.well-known/webfinger` (Account 名基準) | Profile username 基準に置換 (Account 名は 404) |
| `POST /internal/v1/accounts/{id}/sign`、`GET /internal/v1/accounts/{id}/public-key` | **410 Gone** (有効な token でも)。代替の署名 API は作らない。Profile 公開鍵は actor 文書から取得する |
| `/api/v1/accounts/{account_id}/follow|unfollow|followers|following|block|unblock|blocks|mute|unmute|mutes` | **410 Gone**。`/api/v1/profiles/{profile_nanoid}/...` に置換 |

Account 管理 API (作成・取得・更新・無効化・再有効化・組織・通報・移管など) は維持する。

## 5. 本番に mount した route

| route | 内容 |
|---|---|
| `GET /ap/profiles/{profile_nanoid}` | Profile actor 文書 (username と有効鍵の両方を持って初めて解決する) |
| `GET /.well-known/webfinger` | `acct:{username}@{host}` → Profile actor |
| `POST /ap/profiles/{profile_nanoid}/inbox` | Follow / Accept / Reject / Undo(Follow) の受信 (HTTP Signature 検証) |
| `GET /api/v1/profiles/{profile_nanoid}` | Profile identity (所有者のみ) |
| `POST /api/v1/profiles/{profile_nanoid}/follows`、`DELETE .../follows/{follow_id}` | Follow / unfollow |
| `GET .../following`、`GET .../followers` | 所有者のみ |
| `GET|POST .../blocks`、`DELETE .../blocks/{id}`、`GET|POST .../mutes`、`DELETE .../mutes/{id}` | 個別 actor の block / mute (AP `Block` は送らない) |
| `GET .../domain-controls`、`GET|PUT|DELETE .../domain-controls/{domain}` | domain 単位の mute / block (D8P の保存 + D8R の mount)。block の PUT は同一 transaction で対象 domain の既存関係を解除する (下記) |
| `POST /api/v1/profiles/{profile_nanoid}/username` | 一回限りの username 設定 (切替前の既存 Profile 用。切替後は全 Profile が設定済みなので 409) |

### domain block の既存関係の解除 (D8R)

`PUT .../domain-controls/{domain}` で mode=block を保存する (新規作成・mute からの置換・block の再保存) と、**同一 transaction** で、その Profile の対象 domain (完全一致) の actor との有効な関係 (pending 含む・following / followers 両方向) を `blocked` で終了し、必要最小限の後始末を配送 job として予約する。途中で失敗すれば block も含めて全体が rollback される。

| 関係の作者 | 未送信 job | 送信済み・結果不明 |
|---|---|---|
| 自分の Follow | Follow job を取消 (相手は Follow を知らないので何も送らない) | 元 Follow を埋め込んだ `Undo(Follow)`。送信中だった可能性があれば 120 秒後に回す |
| 相手の Follow | Accept job を取消し、`Reject(Follow)` (元 Follow を参照) を予約 | 同上 (送信中の Accept があれば Reject を 120 秒後に回す) |

- 相手の Follow を自分が作者であるかのように Undo しない。AP `Block` は送らない。外部配送の成否は block・解除のローカル適用に影響しない。
- block を解除 (DELETE) しても関係は復元せず、何も配送しない。復元用の一覧も持たない。
- 自 instance の host を block した場合は、同じ host のローカル Profile との関係も解除する (配送なし)。ロックはその相手 Profile も含めて昇順の単一呼び出しで取る。
- ロック順: Profile 行 FOR SHARE → advisory (`lock_profiles`) → job → Follow 行。
- CORS (`CORS_ALLOWED_ORIGINS` 指定時) は PUT / DELETE を許可する。

## 6. ロールバック・運用上の注意

- 切替後の旧モデルへの逆変換は保証しない。復元はバックアップから行う。
- 切替前に旧プロセスの外向き配送 (outbox) に未配送が残っていても、初期化で消える (旧 identity は引き継がない決定)。
- Profile 編集 (icon 変更など) の `Update(Person)` 配送は新モデルにはない (Profile 配送 job の種別は Follow / Accept / Reject / Undo のみ)。後続 unit の課題。
- 配送 worker は `PROFILE_DELIVERY_POLL_INTERVAL_MS` (既定 2000) ごとにキューを処理する。再試行はバックオフ付き (最大 12 回、約 1 日)。
- ローカル開発 DB に旧データと username 未設定の Profile がある場合は migration が拒否する。DB を作り直すか username を設定する。

## 7. 相手 instance の actor 取得 (制約と検証範囲)

- 相手 actor・公開鍵の取得は**署名なしの GET** (`Accept: application/activity+json`) で行う。署名付き取得 (Mastodon の `AUTHORIZED_FETCH` / secure mode) を必須にしている相手の actor は取得できず、その相手への Follow 送信は配送失敗 (再試行後に failed) になる。逆に、こちらの Profile actor は署名なし GET で公開している。署名付き取得への対応 (Profile 鍵での署名 GET) は未実装で、必要になれば後続 unit で扱う。
- 取得先は SSRF 対策で制限される (public な http(s) の URL のみ。loopback・private・link-local・メタデータ宛先・それらへの redirect は拒否)。
- 検証範囲: 実 HTTP での actor / WebFinger / 公開鍵取得 (cache なし) は mock peer E2E (`server/tests/e2e_ap_mock.rs`) と、server の結合テスト (`profile_lifecycle_tests`) が通している。Iceshrimp / Mastodon E2E は、docker 内の自己署名 https の相手を `test-mode` の cache 注入 (`/__test__/cache-actor-key`、`/__test__/cache-remote-actor`) で解決しており、相手の actor 取得そのものは通していない (署名付き Follow 配送・受信 Accept の検証は実 instance で行う)。
