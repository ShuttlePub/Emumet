use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::ProfileId;
use crate::KernelError;
use std::future::Future;

/// Profile の関係 (Follow・個別 block・今後の domain 方針の適用) を **transaction 単位で直列化する**
/// ロックの port。
///
/// 「block 判定 → Follow 保存」と「block 保存 → 既存 Follow の終了」は、行ロック (`FOR SHARE` による所有者
/// 判定) だけでは直列化されない。判定と変更を別々の transaction が interleave すると、block と有効な
/// Follow (とその配送 job) が共存してしまう。これを防ぐため、**block の有無に基づいて判断する、または
/// Follow・block を変更する transaction は、その判断・変更に関わるローカル Profile をすべてこのロックで
/// 先に押さえる**。
///
/// 規約 (P3o の送信側・P3i の受信側・D8G/D8R の domain block 適用が共通に守る):
/// - ロックは transaction の終了 (commit / rollback) で解放される。
/// - 複数の Profile は ID 昇順で取得する (実装がソート・重複除去する)。呼び出し側は同じ transaction で
///   `lock_profiles` を**一度だけ**、関係の読み書きの**前**に呼ぶこと (後から追加で取ると昇順が崩れ、
///   デッドロックしうる)。
/// - リモート actor との関係では、関与するローカル Profile だけを渡す。
pub trait ProfileRelationshipLock: Sync + Send + 'static {
    type Connection: Connection;

    fn lock_profiles(
        &self,
        executor: &mut Self::Connection,
        profiles: &[ProfileId],
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnProfileRelationshipLock: Sync + Send + DependOnDatabaseConnection {
    type ProfileRelationshipLock: ProfileRelationshipLock<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn profile_relationship_lock(&self) -> &Self::ProfileRelationshipLock;
}
