use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::{Nanoid, ProfileBlock, ProfileBlockId, ProfileGraphActor, ProfileId};
use crate::KernelError;
use std::future::Future;

/// Profile 版の個別 block の保存 port。主体 (owner) は常にローカル Profile。
pub trait ProfileBlockRepository: Sync + Send + 'static {
    type Connection: Connection;

    /// 保存する。同じ (owner, target) が既にあれば何もせず false。
    fn insert_if_absent(
        &self,
        executor: &mut Self::Connection,
        block: &ProfileBlock,
    ) -> impl Future<Output = error_stack::Result<bool, KernelError>> + Send;

    fn find(
        &self,
        executor: &mut Self::Connection,
        owner: &ProfileId,
        target: &ProfileGraphActor,
    ) -> impl Future<Output = error_stack::Result<Option<ProfileBlock>, KernelError>> + Send;

    /// owner 自身の設定を nanoid で引く (解除時に block の ID と相手を知るため)。
    fn find_by_nanoid(
        &self,
        executor: &mut Self::Connection,
        owner: &ProfileId,
        nanoid: &Nanoid<ProfileBlock>,
    ) -> impl Future<Output = error_stack::Result<Option<ProfileBlock>, KernelError>> + Send;

    /// owner が target を block しているか。受信・送信の拒否判定は両方向で呼ぶ。
    fn is_blocked(
        &self,
        executor: &mut Self::Connection,
        owner: &ProfileId,
        target: &ProfileGraphActor,
    ) -> impl Future<Output = error_stack::Result<bool, KernelError>> + Send;

    /// owner の block 一覧。ID 昇順、`after` の次から `limit` 件。
    fn list(
        &self,
        executor: &mut Self::Connection,
        owner: &ProfileId,
        after: Option<&ProfileBlockId>,
        limit: i64,
    ) -> impl Future<Output = error_stack::Result<Vec<ProfileBlock>, KernelError>> + Send;

    /// owner 自身の設定だけを nanoid で削除する。他 Profile の設定は消えない。無ければ false。
    fn delete_by_nanoid(
        &self,
        executor: &mut Self::Connection,
        owner: &ProfileId,
        nanoid: &Nanoid<ProfileBlock>,
    ) -> impl Future<Output = error_stack::Result<bool, KernelError>> + Send;
}

pub trait DependOnProfileBlockRepository: Sync + Send + DependOnDatabaseConnection {
    type ProfileBlockRepository: ProfileBlockRepository<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn profile_block_repository(&self) -> &Self::ProfileBlockRepository;
}
