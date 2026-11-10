use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::{Nanoid, ProfileGraphActor, ProfileId, ProfileMute, ProfileMuteId};
use crate::KernelError;
use std::future::Future;

/// Profile 版の個別 mute の保存 port。主体 (owner) は常にローカル Profile。
pub trait ProfileMuteRepository: Sync + Send + 'static {
    type Connection: Connection;

    /// 保存する。同じ (owner, target) が既にあれば何もせず false。
    fn insert_if_absent(
        &self,
        executor: &mut Self::Connection,
        mute: &ProfileMute,
    ) -> impl Future<Output = error_stack::Result<bool, KernelError>> + Send;

    fn find(
        &self,
        executor: &mut Self::Connection,
        owner: &ProfileId,
        target: &ProfileGraphActor,
    ) -> impl Future<Output = error_stack::Result<Option<ProfileMute>, KernelError>> + Send;

    /// owner が target を mute しているか。表示抑制の判定に使う。
    fn is_muted(
        &self,
        executor: &mut Self::Connection,
        owner: &ProfileId,
        target: &ProfileGraphActor,
    ) -> impl Future<Output = error_stack::Result<bool, KernelError>> + Send;

    /// owner の mute 一覧。ID 昇順、`after` の次から `limit` 件。
    fn list(
        &self,
        executor: &mut Self::Connection,
        owner: &ProfileId,
        after: Option<&ProfileMuteId>,
        limit: i64,
    ) -> impl Future<Output = error_stack::Result<Vec<ProfileMute>, KernelError>> + Send;

    /// owner 自身の設定だけを nanoid で削除する。他 Profile の設定は消えない。無ければ false。
    fn delete_by_nanoid(
        &self,
        executor: &mut Self::Connection,
        owner: &ProfileId,
        nanoid: &Nanoid<ProfileMute>,
    ) -> impl Future<Output = error_stack::Result<bool, KernelError>> + Send;
}

pub trait DependOnProfileMuteRepository: Sync + Send + DependOnDatabaseConnection {
    type ProfileMuteRepository: ProfileMuteRepository<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn profile_mute_repository(&self) -> &Self::ProfileMuteRepository;
}
