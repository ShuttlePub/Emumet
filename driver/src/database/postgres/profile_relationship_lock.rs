use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use kernel::interfaces::repository::{DependOnProfileRelationshipLock, ProfileRelationshipLock};
use kernel::prelude::entity::ProfileId;
use kernel::KernelError;
use sqlx::PgConnection;

/// `pg_advisory_xact_lock` による Profile 単位の関係ロック。キーは Profile ID を名前空間つきで
/// ハッシュした bigint で、他の advisory lock の用途とは衝突しない。transaction の終了で解放される。
pub struct PostgresProfileRelationshipLock;

impl ProfileRelationshipLock for PostgresProfileRelationshipLock {
    type Connection = PostgresConnection;

    async fn lock_profiles(
        &self,
        executor: &mut Self::Connection,
        profiles: &[ProfileId],
    ) -> error_stack::Result<(), KernelError> {
        let mut ids: Vec<i64> = profiles.iter().map(|id| *id.as_ref()).collect();
        ids.sort_unstable();
        ids.dedup();
        let con: &mut PgConnection = executor;
        for id in ids {
            sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
                .bind(format!("emumet.profile_relationship:{id}"))
                .execute(&mut *con)
                .await
                .convert_error()?;
        }
        Ok(())
    }

    async fn lock_profile_row_shared(
        &self,
        executor: &mut Self::Connection,
        profile: &ProfileId,
    ) -> error_stack::Result<bool, KernelError> {
        let con: &mut PgConnection = executor;
        let row: Option<i64> =
            sqlx::query_scalar("SELECT id FROM profiles WHERE id = $1 FOR SHARE")
                .bind(profile.as_ref())
                .fetch_optional(con)
                .await
                .convert_error()?;
        Ok(row.is_some())
    }
}

impl DependOnProfileRelationshipLock for PostgresDatabase {
    type ProfileRelationshipLock = PostgresProfileRelationshipLock;

    fn profile_relationship_lock(&self) -> &Self::ProfileRelationshipLock {
        &PostgresProfileRelationshipLock
    }
}
