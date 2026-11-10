# Registration guard contract (E15b / #64)

Kratos **v1.3.1 の実バイナリ**と PostgreSQL を使う隔離実験。
本番 compose / `ory/kratos` / Emumet router は使用・変更しない。
スタブは保護 adapter ではなく故障注入・context 観測用である。

## 再実行

前提: Python 3.11+、OpenSSL CLI、Docker Compose v2 または Podman + compose provider、
公開 image の取得、loopback **18433 / 18434 / 18445** が空いていること。
pip 不要。実メール宛先・実 provider secret を投入しない。同一 project の並列実行は禁止。

**最初に案内するのは今回実行・検証した Nix/native 経路**。
portable Dockerfile の build 自体は未検証であり、下記に別記する。
upstream Rust/Cargo 1.97.0 を使う。`CARGO_BIN` と必要なら `RUSTC`、`PYTHON_BIN`、
`OPENSSL_BIN` に実行可能ファイルを指定できる。

```sh
nix develop --command env NATIVE_APP=1 CONTAINER_ENGINE=podman \
  CARGO_BIN="$(rustup which --toolchain 1.97.0 cargo)" \
  RUSTC="$(rustup which --toolchain 1.97.0 rustc)" \
  bash e2e/registration-guard/run.sh --skip-provider
# 検証時の Nix devshell/Python の HTTPS support 問題を避け、devshell 外で実施:
python3 e2e/registration-guard/provider_probe.py
```

### 代替: portable Dockerfile（未検証）

`NATIVE_APP=1` を省略した既定動作は `Dockerfile.account` による build。
この経路の build は今回未実行のため、上記の検証済み経路と区別する。

```sh
CONTAINER_ENGINE=podman bash e2e/registration-guard/run.sh --skip-provider
# Docker の場合は CONTAINER_ENGINE=docker
python3 e2e/registration-guard/provider_probe.py
```

### 実行範囲

native は実 server を build し、Ubuntu container に read-only `/nix/store` と公開 CA bundle を渡す。
実 Account handler / JWT middleware / DB / 鍵生成・暗号化を通す。
OIDC/JWKS と Keto write/check はスタブで、Hydra/Keto の認可ポリシー自体の試験ではない。
app は host port を公開せず、fixture が専用 network 内で実 HTTP を転送する。
Kratos の HIBP password network check / hook analytics は明示無効化する。

### データと後始末

専用 project は `emumet-registration-guard-e15b`。DB・app log は tmpfs、mail は RAM。
Kratos image の宣言済み volume も tmpfs で覆い、runtime container inspect で
named / anonymous volume が **0** であることを assert する。
生成した鍵・build・生ログ・一時設定は gitignore の `artifacts/` にのみ保存する。

**スクリプトは起動・観測のみ。成功/失敗とも自動 teardown しない。**
利用者がログを確認後、同じ engine で次を明示実行する（volume 削除・prune は不要）。

```sh
podman compose -p emumet-registration-guard-e15b \
  -f e2e/registration-guard/compose.yml \
  --profile account --profile migration down
```

観測中の stack を消さず全 suite を再実行する場合:

```sh
# 既存 DB は削除/初期化せず、一意な新 DB だけを作る。
NATIVE_APP=1 CONTAINER_ENGINE=podman CARGO_BIN=/path/to/upstream/cargo \
  bash e2e/registration-guard/run.sh --fresh-databases --skip-provider
```

`--resume --only account` / `--resume --only native` は中断調査用で、全 suite 成功を意味しない。
`--reuse-empty-stack` は中断した初期セットアップ専用（identity があれば拒否）。
既存 port の強制終了、他 project の変更、DB truncate、volume prune は行わない。
今回は teardown の実行レビューが timeout したため、親指示で stack を保持し operator に手動 cleanup を依頼した。
これは保護の実証結果と区別する。

## 検証方法

- 最初の browser/API 拒否で identity 未作成を確認できなければ直ちに停止。
- 各ケースで admin identity UUID 集合と DB `identities` UUID 集合を突合。
  拒否後は 1 秒後にも再観測し、遅延 persist を検査する。
- browser cookie/CSRF、JSON API、通常 form encoding を UI なしで直接 submit。
- `health/ready` は起動待ち専用。保護 readiness の証拠にしない。
- Turnstile は公式**公開テストキー**6条件。実 credential・有料契約なし。

```sh
python3 -m unittest discover -s e2e/registration-guard -p 'test_*.py' -v
python3 -m compileall -q e2e/registration-guard
bash -n e2e/registration-guard/run.sh
```

## 記録

- [RESULTS.md](RESULTS.md): 条件・期待・観測・証拠の対応、実行範囲・限界
- [CONTRACT.md](CONTRACT.md): E15c/E15d の配線・観測・provider/失敗分類
- `evidence/`: 合成データのみの redacted 観測（session token / credential は除外）
- `hook.jsonnet`: email・verified address・flow ID/type/transient_payload の allowlist
- `stub.py`: allow/deny/timeout/不達/429/503/不正応答、SMTP sink、mock OIDC/Keto
- `run.py`: 実 HTTP / admin / DB assertion と証拠保存（teardown なし）
