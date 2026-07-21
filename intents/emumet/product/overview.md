# Product Overview

## これは何か

Emumet は ShuttlePub のアカウント管理サービス。Event Sourcing + CQRS で構築され、
アカウント・プロフィール・フォロー関係・署名鍵を管理し、ActivityPub 連合との
送受信を中継する。名前は EMU (Extravehicular Mobility Unit) + Helmet 由来。

## ユーザー

- **ShuttlePub 利用者**: Ory Kratos で認証し、Emumet 上に ActivityPub アカウントを持つ。
- **ShuttlePub 本体サービス**: Emumet の内部 API(代理署名など)を利用するサービス間クライアント。
- **インスタンス管理者/モデレーター**: Suspend/Ban 等のモデレーションを行う admin ロール保持者。
- **外部 ActivityPub サーバー**: WebFinger/Actor/Inbox/Outbox を通じて連合するリモート。

## 現状(2026-07 時点の実装インベントリ)

実装済み: Account CRUD + Profile/Metadata、OAuth2 Login/Consent Provider (Hydra 連携)、
WebFinger、Actor、Inbox(Follow/Accept/Undo のみ)、Outbox、Followers/Following、
Follow の送受信配送、HTTP Signature (Cavage 検証 / Cavage+RFC9421 署名)、SSRF 対策、
Suspend/Unsuspend/Ban + Admin/Moderator ロール、内部代理署名 API、
Iceshrimp/Mock peer との E2E。

未実装: ユーザーブロック/ミュート、画像アップロード、Create/Note 等の投稿送受信・
転送、連携先 ShuttlePub サービス設定、通報・ロール割当管理、Mastodon E2E の完成。

## Non-goals

- タイムライン構築・投稿コンテンツの永続化 → ShuttlePub 本体の責務。
- 認証・認可基盤の自前実装 → Ory (Kratos/Hydra) に委譲。
- Stellar を認可サーバーとして構築すること → 凍結済み(decisions/0001 参照)。
