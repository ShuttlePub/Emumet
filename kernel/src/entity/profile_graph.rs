//! Profile 単位の social graph (Follow・個別 block/mute・重複受信記録・配送 job) の保存モデル (PF / P2)。
//!
//! 旧モデルは Account 単位 (`Follow` / `Block` / `Mute` / `OutboxActivity`) だった。ここでは
//! **`ProfileId` を関係の主体** (`ProfileGraphActor::Local`) とし、リモート側は canonical actor URL だけで
//! 表す (`ProfileGraphActor::Remote`)。旧モデルとは独立で、dual-write も変換もしない。
//!
//! 本番 root router・worker にはまだ接続しない (P4 が所有)。型と port は P3o (送信側) / P3i (受信側) が
//! 共通に使う。
//!
//! 型名の `Graph` 接頭辞は、他 unit (P1 の Profile actor 型など) の名前と衝突しないためのもの。

mod activity;
mod actor;
mod delivery_job;
mod follow;
mod inbound_activity;
mod relation;

pub use self::activity::*;
pub use self::actor::*;
pub use self::delivery_job::*;
pub use self::follow::*;
pub use self::inbound_activity::*;
pub use self::relation::*;
