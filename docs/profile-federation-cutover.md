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
| 0 | `20261120000000_profile_federation_all_profiles_precheck.sql`: **イベント上の全既存 Profile**（無効化・未射影を含む）の username を検査 | migration | 未設定が 1 件でもあれば起動失敗。P4 migration は 1 件も適用されず、旧データ・台帳に変更なし |
| 1 | `20261120000001_profile_social_graph.sql`: Profile 単位の関係・配送テーブル (`profile_follows` `profile_blocks` `profile_mutes` `profile_inbound_activities` `profile_delivery_jobs` `profile_delivery_attempts`) を追加 (非破壊) | migration | 起動失敗。DB は変わらない (transaction) |
| 2 | `20261120000002_reset_legacy_federation.sql`: **前提検査** (username 未設定の Profile が 1 件でもあれば `RAISE EXCEPTION`) → 旧連合データの `TRUNCATE` → 台帳 `profile_federation_resets` に `legacy-account-federation-v1` を記録。全て 1 transaction | migration | 起動失敗。**何も消えず台帳も残らない** (切替拒否) |
| 2b | `20261121000001_profile_signing_keys_survive_deactivation.sql`: `profile_signing_keys.profile_id` の外部キー (ON DELETE CASCADE) を外す (非破壊)。Account 無効化の projection が `profiles` 行を消しても Profile 鍵は保持され、再有効化で行が再 materialize されると同じ鍵で復帰する (下記「Account の状態と Profile」) | migration | 起動失敗。DB は変わらない |
| 3 | Profile 鍵の一回限り生成 (`server/src/cutover.rs` → `EnsureProfileSigningKeysUseCase`): **有効な鍵を持たない全既存 Profile（イベントにのみ残る無効化・未射影 Profile も含む）にだけ**鍵を作る。1 transaction・advisory lock で直列化 | 起動処理 (`main`、root router / worker より前) | 起動失敗 (鍵のない Profile を actor として公開しない) |
| 4 | projection worker・webhook worker・**Profile 配送 worker** を起動し、root router を serve | `main` | — |

### Account の状態と Profile (切替後の挙動)

- **停止 (suspend)・BAN 中**の Account の Profile は actor ではない: actor 文書・WebFinger は 404、inbox は 404 (相手には「存在しない」と見える)、その Profile からの新規 Follow は 403、停止中の Profile を対象にした Follow は 400。**Profile 行・関係・配送 job は残る**: 配送 worker は `claim_due` でその Profile の job を取得しないだけで、キューに待機し (試行回数も消費しない)、解除すると送信される。ルールは `AccountQuery::find_by_id` と同じ (期限付き停止は期限が切れれば自動で解除)。
- **無効化 (deactivate) 中**は上記に加えて **Profile 行そのものが projection から消える**。`profile_follows` `profile_blocks` `profile_mutes` `profile_delivery_jobs` などは Profile 行への外部キー (ON DELETE CASCADE) で**一緒に消える** (待機ではなく消去)。再有効化で Profile 行は戻るが、消えた関係・未送信の job は戻らない。保持されるのは Profile の identity (id・nanoid・username) と鍵 (`profile_signing_keys` は外部キーを持たない) だけ。
- 停止・BAN の解除 (unsuspend / unban / 期限切れ) で、**同じ actor URL・同じ鍵**のまま復帰し、待機していた job が送られる。再有効化でも同じ actor URL・鍵で復帰するが、上記のとおり消えた関係・job は戻らない。
- **無効化→再有効化**: 無効化で Account の projection が `profiles` 行を削除する。再有効化の projection (Account projector) が、その Account が所有する Profile を**全 event 履歴から再 materialize** する (tailing の窓 (直近 100 件) に依存しない)。Profile の id・nanoid・username と鍵は変わらない。
- **切替前から無効化中**の Account も対象: `profile_events` の Created が保持する不変の id・nanoid から、行を復元せず起動時に鍵だけを生成する。username は Created または後続 UsernameSet から検査する。無効化状態を変えたり actor を公開したりしない。再有効化時は全履歴の復元だけで既存鍵に結び付き、再起動・遅延鍵生成は不要。切替後の新規 Profile は作成 transaction 内で鍵を持つため、再有効化 projector に crypto 処理は追加しない。
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

1. **全既存 Profile に username が設定済み**であること (FN `profile-identity-web-adaptation` のマージ後、所有者 API または §3.1 の専用復旧コマンドで設定)。
   未設定が残っていると、**最初の事前検査 migration (`20261120000000`)**が `profile federation cutover refused: N profile(s) have no username ...` で abort する (`20261120000001`・`20261120000002` の射影行検査も安全弁として維持する)。
   migration は P4 の 1 件も適用されず、DB は切替前のまま・旧データも消えない。ただし**新バイナリは migration に失敗して起動しない**ので、復旧は §3.1 の手順で行う。
   **全既存 Profile = `profiles` の行と `profile_events` の全 stream の和集合**。行数だけの確認では無効化 Profile を取りこぼすため、下の専用診断の `missing_usernames` と `problems` がともに 0 であることを確認する。
   DB の `NOT NULL` 化は行わない (旧イベント列からの projection 再構築で username を持たない Created イベントが残りうるため)。作成 API 側で必須化している。
2. master key password (`/run/secrets/master-key-password`、開発では `./master-key-password`) が新バイナリから読めること (鍵の生成・署名に使う)。
3. DB のバックアップを取得済みであること。
4. `PUBLIC_BASE_URL` が運用値であること (actor URL・key id はこの値から作られ、鍵 id の URI に焼き込まれる)。
5. projection が追いついていること (`projection_checkpoints`)。ただし無効化中の Account の Profile は正常でも行がない。鍵生成はイベント上の identity も対象なので、そのような Profile を次回起動まで取りこぼさない。旧プロセス停止中に migration → 全既存鍵生成を完了してから新 worker・route を開始する。

事前確認は §3.1 で配布・ビルドする **専用バイナリ**を使う（通常サーバーではない）。

```sh
# DATABASE_URL は対象 DB を指す運用者の環境設定。接続文字列をログに出さない。
./profile-cutover-recovery diagnose > before-recovery.json
```

診断は REPEATABLE READ / READ ONLY transaction で全履歴を読み、行だけでは見えない無効化 Profile、
後続の AccountTransferred を含む現在の所有 Account（id・nanoid・無効化状態）、Profile の id・nanoid・
username・期待 version・射影行の有無を JSON で出す。id/version は精度を失わない文字列。

起動時は `PostgresDatabase::migrate` が同じ全既存 Profile 検査を**毎回 migration より前に**実行する（空 DB / username 列導入前は versioned 検査まで待つ）。最初の検査だけ成功して別の migration が失敗した後に、旧バイナリで未設定 Profile が増えた場合も再試行で拒否する。

追加検査は既存 migration の checksum を変えないため P4 より前に採番している。P4 適用済み DB にも sqlx は未適用のこの番号を適用するが、reset 台帳があれば切替専用の検査はスキップし、再消去しない。起動時の鍵生成は引き続き冪等で、既に切替済みの DB に残る鍵なしの無効化 Profile も補完する。

## 3. 切替手順

1. **旧機構を停止**する: 旧 inbox・Account 単位の関係書込 (旧 follow/block/mute API)・旧配送 worker を含む**旧プロセス全体**を停止する。新バイナリの起動前に必ず止める (migration 中に旧プロセスが旧テーブルへ書くと検証できない)。
2. バックアップを取り、事前条件を確認する (§2)。
3. **新バイナリを 1 プロセスだけ起動**する。起動ログで次を確認する:
   - migration の適用 (拒否された場合は `profile federation cutover refused` で終了する。username を設定して再起動する。何も変わっていない)
   - `generated the one-time actor keys of Profiles that had none` (created = 今回新規生成した数。無効化・未射影 Profile も含む) または `every Profile already has an actor key`
4. **スモーク確認**:
   - `GET /.well-known/webfinger?resource=acct:{username}@{PUBLIC_BASE_URL の host[:port]}` → 200、`self` link が `{PUBLIC_BASE_URL}/ap/profiles/{nanoid}`
   - `GET /ap/profiles/{nanoid}` (Accept: `application/activity+json`) → 200、`publicKey.id` が `{actor}#main-key`
   - `GET /ap/accounts/{account_nanoid}` → **410 Gone** (新 actor へ redirect しない)、`/internal/v1/accounts/{id}/sign` → 410
   - 外部 instance (Mastodon / Iceshrimp) から Profile actor を検索して Follow → `GET /api/v1/profiles/{nanoid}/followers` に載り、Accept が配送される
   - `POST /api/v1/profiles/{nanoid}/follows` で外部 actor を Follow → 相手の inbox に署名付き Follow が届く
5. 旧バイナリへ戻さない。問題があればバックアップから復元する。

### 3.1 username 検査で拒否された場合の復旧

拒否時点で **P4 migration が 1 件も適用されていない**なら、旧連合データ・旧鍵は保持され、
バックアップ復元は不要。新サーバーも旧サーバーも起動せず、専用コマンドで解消する。
古い無効化 Account の再有効化は、P4 前 projector の deleted-row lookup / tailing-window 問題で
Profile が戻らない場合があるため、**復旧の前提にしない**。

#### 準備・診断（読み取り専用）

1. 全ての旧/新サーバー・worker を停止し、ingress も閉じたままにする。バックアップ、拒否ログ、
   操作者・変更管理 ticket、対象所有者の同意を確保する。本番での復旧書込も運用者の承認に従う。
   診断には無効化中 Account の対応関係も含むため、`umask 077` の管理 shell を使い、出力は非公開で保管する。
2. レビュー済みの recovery unit の専用成果物だけを配布する。ソースから作る場合:

   ```sh
   cargo build --locked --release -p driver --bin profile-cutover-recovery
   # 配布するファイル: target/release/profile-cutover-recovery
   ```

   このコマンドは SQLx で直接接続し、**AppModule・通常サーバー・migration・鍵生成・配送を起動しない**。
   `cargo run -p server` や P4 バイナリの起動を復旧の代用にしない。接続先 `DATABASE_URL` を運用者が
   確認する。診断だけなら読み取り権限の DB role でもよい（通常の pre-P4 FN/P1 schema が必要）。
3. `./profile-cutover-recovery diagnose > before-recovery.json 2> diagnose.stderr` を実行する。
   全 Profile を列挙するため `username: null` の対象を確認し、全履歴上の現在の所有 Account と照合する。
   無効化中・行なしでも対象。`problem` があれば自動設定しない（後述）。

#### 個別解消（既定 dry-run → 明示 write）

4. 各対象について、所有者と**一度だけ・変更不可**の username を合意する。Account 名から自動導出しない。
   未設定 Profile の除外や削除によって検査をすり抜けない。診断の文字列 id/version をそのまま使い、
   操作者・理由・一意の request ID を用意する。

   ```sh
   # 値は before-recovery.json と合意済みの変更管理記録から運用者が指定する。
   profile_id='PROFILE_ID'
   expected_version='PROFILE_VERSION'
   username='AGREED_USERNAME'
   request_id='TICKET-UNIQUE-PROFILE-REQUEST'
   operator='OPERATOR'
   reason='OWNER_APPROVAL_AND_TICKET'
   ./profile-cutover-recovery set-username \
     --profile-id "$profile_id" --expected-version "$expected_version" \
     --username "$username" --request-id "$request_id" \
     --operator "$operator" --reason "$reason" > dry-run.json 2> dry-run.stderr
   ```

   成功（終了 0、`status: dry_run`）と対象/所有者・`request_id`・`requested_username` を確認してから、
   **同じ引数に `--write` だけを追加**する（`profile` は設定前の状態）:

   ```sh
   ./profile-cutover-recovery set-username \
     --profile-id "$profile_id" --expected-version "$expected_version" \
     --username "$username" --request-id "$request_id" \
     --operator "$operator" --reason "$reason" --write > applied.json 2> applied.stderr
   ```

   `status: applied` なら完了。既存 `UsernameSet` イベントと、存在する Profile 行の username/version、
   監査行を **1 transaction** で保存する。Profile 行がなければ復元せず、Account の状態・鍵・旧連合データ・
   checkpoint は一切変えない。旧版もこのイベント形式を解釈できる。
   書込 role には対象イベント/射影テーブルの読書き・ロックと監査表の作成/書込権限が必要。
   `WORKER_ID` は 0..1023（既定 0）。他の writer を全停止し、復旧コマンドも原則 1 つずつ実行する。
5. 応答を失った場合は **request ID・期待 version・操作者・理由も含め同じ引数のまま**再実行する。
   `status: already_applied` は保存済み結果であり、イベント・監査・鍵を増やさない。
   同じ request ID の引数変更は拒否される。期待 version が変わったら再診断・所有者再確認のうえ新しい
   request ID を使う。username の予約は無効化 Profile のイベントも含み、一回限り/一意 index が最終防衛線。
   ロック取得は 5 秒で失敗させる（writer が残っていないか調べ、盲目的に繰り返さない）。

#### 再診断・再試行・監査

6. `./profile-cutover-recovery diagnose > after-recovery.json 2> after-recovery.stderr` を実行する。
   `missing_usernames: 0` **かつ** `problems: 0` を確認する。Account は無効化中のままでよく、
   再有効化・射影の再構築は不要。旧サーバーを再開すると未設定 Profile が増えうるので停止したままとする。
   その後、**別途の本番切替承認ゲート**に従い §3 の手順 3 から再試行する。
   切替時の全既存 Profile 検査・全鍵生成は変更しておらず、未設定が 1 件でも残れば引き続き拒否する。
7. 診断・dry-run・write の stdout/stderr、終了コード、実行した専用バイナリの commit SHA を ticket に保管する。
   成功時の DB 監査は `profile_cutover_recovery_audit`（request ID、operator/reason/期待 version/username を含む
   request、現在の所有者と前後 version を含む result、DB timestamp）。必要なら読み取り専用で確認できる:

   ```sql
   SELECT request_id, request, result, recorded_at
   FROM profile_cutover_recovery_audit ORDER BY recorded_at, request_id;
   ```

   この監査表だけは最初の明示 write の transaction 内で非破壊作成される。diagnose/dry-run は表も作らない。
   失敗は非ゼロ終了で DB transaction は rollback（イベントだけ/監査だけの部分成功なし）。拒否された試行は
   DB 成功監査には残らないため stderr/終了コードも保管する。成功監査は P4 初期化の対象外で保持される。

**停止条件**: schema が FN/P1 より前、P4 migration が既に記録済み（前提検査だけ成功した途中状態を含む）、
reset 台帳あり、履歴が不完全/不正、行とイベントの不整合、所有者を合意できない場合はコマンドが拒否するか
診断の `problem` を示す。手書き SQL で行・イベント・migration 台帳を修正したり、旧/新サーバー起動で
迂回しない。原因とログをリードに報告し、途中適用や別 schema の復旧は個別判断する。
通常の未設定拒否は上の診断・イベント設定で解消できるが、破損した履歴の自動修復や projector の backport は
この専用コマンドの対象外。

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
| `GET|POST .../blocks`、`DELETE .../blocks/{id}`、`GET|POST .../mutes`、`DELETE .../mutes/{id}` | 個別 actor の block / mute (個別 block は remote actor へ AP `Block` を送り、解除は `Undo(Block)`。domain block は送らない) |
| `GET .../domain-controls`、`GET|PUT|DELETE .../domain-controls/{domain}` | domain 単位の mute / block (D8P の保存 + D8R の mount)。block の PUT は同一 transaction で対象 domain の既存関係を解除する (下記) |
| `POST /api/v1/profiles/{profile_nanoid}/username` | 一回限りの username 設定 (切替前の既存 Profile 用。切替後は全 Profile が設定済みなので 409) |

### 個別 actor の block の AP `Block` (D8R、ユーザー決定)

個別 actor の block (`POST .../blocks`) は、双方向の Follow 解除と `Undo(Follow)` / `Reject(Follow)` に加えて、remote actor の inbox へ `Block` (`object` = 相手 actor、ID は `{actor}/blocks/{block ID}`) を同一 transaction で予約する。解除 (`DELETE .../blocks/{id}`) は、未送信の `Block` job を取り消す (`Undo` なし)、送信済み・結果不明なら元 Block を埋め込んだ `Undo(Block)` を同じ宛先へ予約する (送信中だった可能性があれば元の送信が届き得る時間の後)。ローカル Profile 相手は配送しない。相手を解決できなくても block と解除は適用され、解除した Follow から分かる inbox があればそこへ、無ければ `Block` だけ予約されない。受信した `Block` / `Undo(Block)` は関係を変えない (P3i)。

block / unblock を繰り返しても最新の意思が相手側で最後になるよう、状態を変える操作 (block・unblock) は先に **同じ actor 対の未配送の `Block` / `Undo(Block)` (配送中・再試行待ちを含む) を取り消す** (相手は actor 対で block を管理し、`Undo(Block)` は Block の ID を見ないため)。actor 対の操作は transaction 内の排他ロックで直列化する。取消は actor を解決できたかに依らない (宛先 inbox は解決できたもの → この actor 対の最新 job の inbox → 解除した Follow から分かるもの)。

取消の瞬間に送信中だった job (取消と同じ行ロックの下で読んだ状態・lease の期限。読取後に claim / renew で延びた期限を含む) があれば、actor 対 (送信元 Profile × 相手 actor) の下限 `profile_block_settle.settle_not_before` を、`max(取消後に取り直した時刻 + 120 秒, 取消時点の lease の期限, 既存値)` まで引き上げる (`GREATEST`。単調非減少)。以後この actor 対に予約する `Block` / `Undo(Block)` は必ずこの下限以上に始めるので、取消に時間が掛かっても・操作を何度繰り返しても前倒しされない。`unblock` は、相手側に block が残り得る (試行済みの `Block` があり、それを打ち消す `Undo` が配送済みでない) ときだけ、最新のその `Block` を埋め込む `Undo(Block)` を予約する。Follow の `Undo` / `Reject` の送信中判定も、同じく取消の瞬間の状態と lease を使う。

### domain block の既存関係の解除 (D8R)

`PUT .../domain-controls/{domain}` で mode=block を保存する (新規作成・mute からの置換・block の再保存) と、**同一 transaction** で、その Profile の対象 domain (完全一致) の actor との有効な関係 (pending 含む・following / followers 両方向) を `blocked` で終了し、必要最小限の後始末を配送 job として予約する。途中で失敗すれば block も含めて全体が rollback される。

| 関係の作者 | 未送信 job | 送信済み・結果不明 |
|---|---|---|
| 自分の Follow | Follow job を取消 (相手は Follow を知らないので何も送らない) | 元 Follow を埋め込んだ `Undo(Follow)`。送信中だった可能性があれば 120 秒後に回す |
| 相手の Follow | Accept job を取消し、`Reject(Follow)` (元 Follow を参照) を予約 | 同上 (送信中の Accept があれば Reject を 120 秒後に回す) |

- 相手の Follow を自分が作者であるかのように Undo しない。**domain block は AP `Block` を送らない** (個別 actor の block は送る。下記)。外部配送の成否は block・解除のローカル適用に影響しない。
- block を解除 (DELETE) しても関係は復元せず、何も配送しない。復元用の一覧も持たない。
- 自 instance の host を block した場合は、同じ host のローカル Profile との関係も解除する (配送なし)。ロックはその相手 Profile も含めて昇順の単一呼び出しで取る。
- ロック順: Profile 行 FOR SHARE → advisory (`lock_profiles`) → block 保存 → Follow 行 (`end_active_between`) → job 行 (取消・予約)。配送前ガードは job → Follow の順に更新するが、同じ Profile の advisory ロックを先に取るため、この解除 (と unfollow) とは直列化され行ロックは循環しない。`renew_lease` / `record_attempt` は job 行だけを触る。
- 後始末 (Undo / Reject) の配送開始は、元 job の取消が済んだ後に取り直した時刻 + 120 秒 (lease の期限が分かればそれ以降)。解除バッチや transaction の開始時刻から数えない (取消までに時間が掛かると、元の送信より先に後始末が着地し得るため)。
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
