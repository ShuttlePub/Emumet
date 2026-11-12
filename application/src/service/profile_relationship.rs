//! Profile 単位の関係の送信側 (P3o): 関係 use case・配送 worker・activity の組み立て。
//!
//! 本番の root router・main には接続しない (P4 が所有)。テストが直接組み立てた構成で検証する。

pub mod activity;
mod delivery;
pub(crate) mod domain_block;
mod relationship;

pub use delivery::{
    DeliverProfileActivitiesUseCase, ProfileDeliveryCycleReport, ProfileDeliveryEnv,
};
pub(crate) use relationship::Conn;
pub use relationship::{ProfileRelationshipUseCase, RelationshipDeps};

#[cfg(test)]
mod tests;
