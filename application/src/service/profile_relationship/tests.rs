//! 実 DB (本番 migration を全て適用した独立 DB)・fake peer (HTTP mock) を使う P3o の受け入れテスト。
//!
//! 各テストは専用の独立 DB を作るので、互いに干渉せず TRUNCATE も要らない。DB 部分は
//! `DATABASE_URL` を設定した実 DB が必要 (`test_with::env`)。

mod delivery;
mod domain_block;
mod domain_guard;
mod relationship;
mod support;
