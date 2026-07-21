# Interview: initial shaping (2026-07-22)

chat-first セッションで記録。intent-cli interview セッション機構が CLI 0.5.0 で
確立できなかったため、同等の Q/A を本ファイルに直接記録した。

参照: 実装インベントリは `server/src/route/`, `kernel/src/entity/` 等のコード調査、
および https://docs.shuttle.pub/docs/emumet (features.md / data-structure.md) に基づく。

## Q1: 投稿の中継役(Create/Note の受信→転送、署名付き外部配送)は Emumet のスコープか?

**Answer**: 中継は Emumet のスコープ。このサービスの本質は ShuttlePub サービス全体で
使えるアカウントを管理すること。転送先は Stellar ではない(Stellar は認可サーバーに
なるはずだったサービス)。ShuttlePub(本体)がタイムラインを構築する SNS 本体サービスに
なる。サービス全体の分散思想と ActivityPub 仕様のバランスを取るため、アカウントの
住所(acct)としては Emumet のドメインを差し、連携している ShuttlePub サーバーに
投稿を転送する。逆方向(ShuttlePub 発の投稿)は、署名用の鍵を保持する Emumet が
代理で署名して外部に配送する。

## Q2: StellarAccount イベント定義(「利用するメインのShuttlePubサービスの保存」)は有効か?

**Answer**: 有効。Stellar システムの構築は凍結され、一部責務は Emumet が持ち、
認可まわりは Ory (Kratos/Hydra) で代替することになった。ただし「メインサービスの保存」
というより「アカウントごとの連携先 ShuttlePub サービスを設定し、自分宛ての投稿を
そこに流す」形になる。

## Q3: 未実装イベント群(Moderator 割当、HostModeration、AccountReport 等)は実装するか?

**Answer**: ロールシステムは一部だけ実装済みだが、まだ不足している。サービスの
アドミン達がモデレーションをちゃんとできる状態まで実装したい。

## Q4: 最初に取り組む packet は?

**Answer**: ミュート/ブロック機能。ミュート/ブロックと画像アップロード以外は
現状でかなりできている認識。
