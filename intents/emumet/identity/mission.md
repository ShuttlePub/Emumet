---
facets: [vocabulary, invariant]
---

# Mission

## Mission statement

ShuttlePub サービス群全体で共有されるアカウントの唯一の正本(source of truth)を
管理し、ActivityPub 上のアカウントの住所(acct)を Emumet ドメインで提供する。
投稿の署名鍵を預かり、連合との送受信を代理・中継することで、本体サービス
(ShuttlePub)がタイムライン構築に集中できるようにする。

## Vision

- ユーザーは 1 つの Emumet アカウントで、任意の連携 ShuttlePub サーバーから
  ActivityPub 連合に参加できる。
- 外部から見たアカウントの住所は常に Emumet ドメインであり、利用する本体サービスを
  変えても住所・フォロワー関係・署名鍵は維持される。
- 認証・認可は Ory (Kratos/Hydra) に委譲し、Emumet はアカウント管理と連合中継に
  責務を限定する。

## Values / principles

- **Event Sourcing + CQRS**: アカウント系の状態変更はすべてイベントとして記録する。
- **責務の分離**: タイムライン構築・投稿永続化は ShuttlePub 本体、認証認可は Ory、
  アカウント・鍵・連合中継は Emumet。
- **連合ファースト**: ActivityPub 仕様との整合を優先し、分散思想とのバランスは
  転送モデル(住所=Emumet、本体=ShuttlePub)で解決する。
- **安全性**: SSRF 対策・HTTP Signature 検証など、連合通信の安全性を手抜かない。

## Glossary

- **ShuttlePub(本体)**: タイムラインを構築する SNS 本体サービス。Emumet と連携する。
- **Stellar**: 認可サーバーになる予定だったサービス。現在凍結。責務は Emumet と Ory に分散。
- **Ory (Kratos/Hydra)**: 認証(Kratos)と OAuth2/OIDC 認可(Hydra)の外部サービス。
- **連携先 ShuttlePub サービス**: アカウントごとに設定する、自分宛て投稿の転送先。
- **代理署名**: ShuttlePub 発の投稿に対し、Emumet が保持する秘密鍵で HTTP Signature を付与すること。
- **acct / 住所**: `user@emumet-domain` 形式の ActivityPub アカウント識別子。
