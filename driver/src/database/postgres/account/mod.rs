mod row;

pub use row::PostgresAccountReadModel;
use row::{account_from_row, AccountRow};

use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use error_stack::Report;
use kernel::interfaces::read_model::{AccountReadModel, DependOnAccountReadModel};
use kernel::prelude::entity::{
    Account, AccountId, AccountName, AccountStatus, AuthAccountId, Nanoid,
};
use kernel::KernelError;
use sqlx::types::time::OffsetDateTime;
use sqlx::PgConnection;

impl AccountReadModel for PostgresAccountReadModel {
    type Executor = PostgresConnection;

    async fn find_by_id(
        &self,
        executor: &mut Self::Executor,
        id: &AccountId,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, AccountRow>(
            //language=postgresql
            r#"
            SELECT id, name, is_bot, deleted_at, version, nanoid, created_at,
                   suspended_at, suspend_expires_at, suspend_reason, banned_at, ban_reason
            FROM accounts
            WHERE id = $1 AND deleted_at IS NULL
              AND banned_at IS NULL
              AND (suspended_at IS NULL OR (suspend_expires_at IS NOT NULL AND suspend_expires_at <= now()))
            "#,
        )
        .bind(id.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()
        .map(|option| option.map(Account::from))
    }

    async fn find_by_auth_id(
        &self,
        executor: &mut Self::Executor,
        auth_id: &AuthAccountId,
    ) -> error_stack::Result<Vec<Account>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, AccountRow>(
            //language=postgresql
            r#"
            -- Intentionally does NOT filter suspended/banned: allows account owners
            -- to see their own accounts' moderation status via the listing endpoint.
            SELECT accounts.id, name, is_bot, deleted_at, version, nanoid, created_at,
                   suspended_at, suspend_expires_at, suspend_reason, banned_at, ban_reason
            FROM accounts
            INNER JOIN auth_emumet_accounts ON auth_emumet_accounts.emumet_id = accounts.id
            WHERE auth_emumet_accounts.auth_id = $1 AND deleted_at IS NULL
            "#,
        )
        .bind(auth_id.as_ref())
        .fetch_all(con)
        .await
        .convert_error()
        .map(|rows| {
            rows.into_iter()
                .map(|row| account_from_row(row, true))
                .collect()
        })
    }

    async fn find_by_name(
        &self,
        executor: &mut Self::Executor,
        name: &AccountName,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, AccountRow>(
            //language=postgresql
            r#"
            SELECT id, name, is_bot, deleted_at, version, nanoid, created_at,
                   suspended_at, suspend_expires_at, suspend_reason, banned_at, ban_reason
            FROM accounts
            WHERE name = $1 AND deleted_at IS NULL
              AND banned_at IS NULL
              AND (suspended_at IS NULL OR (suspend_expires_at IS NOT NULL AND suspend_expires_at <= now()))
            "#,
        )
        .bind(name.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()
        .map(|option| option.map(Account::from))
    }

    async fn find_by_nanoid(
        &self,
        executor: &mut Self::Executor,
        nanoid: &Nanoid<Account>,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, AccountRow>(
            //language=postgresql
            r#"
            SELECT id, name, is_bot, deleted_at, version, nanoid, created_at,
                   suspended_at, suspend_expires_at, suspend_reason, banned_at, ban_reason
            FROM accounts
            WHERE nanoid = $1 AND deleted_at IS NULL
              AND banned_at IS NULL
              AND (suspended_at IS NULL OR (suspend_expires_at IS NOT NULL AND suspend_expires_at <= now()))
            "#,
        )
        .bind(nanoid.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()
        .map(|option| option.map(Account::from))
    }

    async fn find_by_nanoids(
        &self,
        executor: &mut Self::Executor,
        nanoids: &[Nanoid<Account>],
    ) -> error_stack::Result<Vec<Account>, KernelError> {
        let con: &mut PgConnection = executor;
        let nanoid_strs: Vec<&str> = nanoids.iter().map(|n| n.as_ref().as_str()).collect();
        sqlx::query_as::<_, AccountRow>(
            //language=postgresql
            r#"
            SELECT id, name, is_bot, deleted_at, version, nanoid, created_at,
                   suspended_at, suspend_expires_at, suspend_reason, banned_at, ban_reason
            FROM accounts
            WHERE nanoid = ANY($1) AND deleted_at IS NULL
              AND banned_at IS NULL
              AND (suspended_at IS NULL OR (suspend_expires_at IS NOT NULL AND suspend_expires_at <= now()))
            "#,
        )
        .bind(&nanoid_strs)
        .fetch_all(con)
        .await
        .convert_error()
        .map(|rows| rows.into_iter().map(Account::from).collect())
    }

    async fn create(
        &self,
        executor: &mut Self::Executor,
        account: &Account,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            //language=postgresql
            r#"
            INSERT INTO accounts (id, name, is_bot, version, nanoid, created_at)
            VALUES ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(account.id().as_ref())
        .bind(account.name().as_ref())
        .bind(account.is_bot().as_ref())
        .bind(account.version().as_ref())
        .bind(account.nanoid().as_ref())
        .bind(account.created_at().as_ref())
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }

    async fn update(
        &self,
        executor: &mut Self::Executor,
        account: &Account,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        let (suspended_at, suspend_expires_at, suspend_reason, banned_at, ban_reason) =
            match account.status() {
                AccountStatus::Active => (None, None, None, None, None),
                AccountStatus::Suspended {
                    reason,
                    suspended_at,
                    expires_at,
                } => (
                    Some(*suspended_at),
                    *expires_at,
                    Some(reason.clone()),
                    None,
                    None,
                ),
                AccountStatus::Banned { reason, banned_at } => {
                    (None, None, None, Some(*banned_at), Some(reason.clone()))
                }
            };
        let result = sqlx::query(
            //language=postgresql
            r#"
            UPDATE accounts
            SET name = $2, is_bot = $3, version = $4, deleted_at = $5,
                suspended_at = $6, suspend_expires_at = $7, suspend_reason = $8,
                banned_at = $9, ban_reason = $10
            WHERE id = $1
            "#,
        )
        .bind(account.id().as_ref())
        .bind(account.name().as_ref())
        .bind(account.is_bot().as_ref())
        .bind(account.version().as_ref())
        .bind(account.deleted_at().as_ref().map(|d| d.as_ref()))
        .bind(suspended_at)
        .bind(suspend_expires_at)
        .bind(suspend_reason)
        .bind(banned_at)
        .bind(ban_reason)
        .execute(con)
        .await
        .convert_error()?;
        if result.rows_affected() == 0 {
            return Err(Report::new(KernelError::NotFound)
                .attach_printable("Target account not found for update"));
        }
        Ok(())
    }

    async fn deactivate(
        &self,
        executor: &mut Self::Executor,
        account_id: &AccountId,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        let result = sqlx::query(
            //language=postgresql
            r#"
            UPDATE accounts
            SET deleted_at = CASE WHEN deleted_at IS NULL THEN now() ELSE deleted_at END
            WHERE id = $1
            "#,
        )
        .bind(account_id.as_ref())
        .execute(con)
        .await
        .convert_error()?;
        if result.rows_affected() == 0 {
            return Err(Report::new(KernelError::NotFound)
                .attach_printable("Target account not found for deactivate"));
        }
        Ok(())
    }

    async fn unlink_all_auth_accounts(
        &self,
        executor: &mut Self::Executor,
        account_id: &AccountId,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            //language=postgresql
            r#"
            DELETE FROM auth_emumet_accounts WHERE emumet_id = $1
            "#,
        )
        .bind(account_id.as_ref())
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }

    async fn link_auth_account(
        &self,
        executor: &mut Self::Executor,
        account_id: &AccountId,
        auth_account_id: &AuthAccountId,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            //language=postgresql
            r#"
            INSERT INTO auth_emumet_accounts (emumet_id, auth_id) VALUES ($1, $2)
            "#,
        )
        .bind(account_id.as_ref())
        .bind(auth_account_id.as_ref())
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }

    async fn find_by_id_unfiltered(
        &self,
        executor: &mut Self::Executor,
        id: &AccountId,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, AccountRow>(
            //language=postgresql
            r#"
            SELECT id, name, is_bot, deleted_at, version, nanoid, created_at,
                   suspended_at, suspend_expires_at, suspend_reason, banned_at, ban_reason
            FROM accounts
            WHERE id = $1 AND deleted_at IS NULL
            "#,
        )
        .bind(id.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()
        .map(|option| option.map(|row| account_from_row(row, true)))
    }

    async fn find_by_nanoid_unfiltered(
        &self,
        executor: &mut Self::Executor,
        nanoid: &Nanoid<Account>,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, AccountRow>(
            //language=postgresql
            r#"
            SELECT id, name, is_bot, deleted_at, version, nanoid, created_at,
                   suspended_at, suspend_expires_at, suspend_reason, banned_at, ban_reason
            FROM accounts
            WHERE nanoid = $1 AND deleted_at IS NULL
            "#,
        )
        .bind(nanoid.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()
        .map(|option| option.map(|row| account_from_row(row, true)))
    }

    async fn find_by_nanoids_unfiltered(
        &self,
        executor: &mut Self::Executor,
        nanoids: &[Nanoid<Account>],
    ) -> error_stack::Result<Vec<Account>, KernelError> {
        let con: &mut PgConnection = executor;
        let nanoid_strs: Vec<&str> = nanoids.iter().map(|n| n.as_ref().as_str()).collect();
        sqlx::query_as::<_, AccountRow>(
            //language=postgresql
            r#"
            SELECT id, name, is_bot, deleted_at, version, nanoid, created_at,
                   suspended_at, suspend_expires_at, suspend_reason, banned_at, ban_reason
            FROM accounts
            WHERE nanoid = ANY($1) AND deleted_at IS NULL
            "#,
        )
        .bind(&nanoid_strs)
        .fetch_all(con)
        .await
        .convert_error()
        .map(|rows| {
            rows.into_iter()
                .map(|row| account_from_row(row, true))
                .collect()
        })
    }

    async fn suspend(
        &self,
        executor: &mut Self::Executor,
        account_id: &AccountId,
        reason: &str,
        expires_at: Option<OffsetDateTime>,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            //language=postgresql
            r#"
            UPDATE accounts
            SET suspended_at = now(), suspend_expires_at = $2, suspend_reason = $3
            WHERE id = $1
            "#,
        )
        .bind(account_id.as_ref())
        .bind(expires_at)
        .bind(reason)
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }

    async fn unsuspend(
        &self,
        executor: &mut Self::Executor,
        account_id: &AccountId,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            //language=postgresql
            r#"
            UPDATE accounts
            SET suspended_at = NULL, suspend_expires_at = NULL, suspend_reason = NULL
            WHERE id = $1
            "#,
        )
        .bind(account_id.as_ref())
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }

    async fn ban(
        &self,
        executor: &mut Self::Executor,
        account_id: &AccountId,
        reason: &str,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            //language=postgresql
            r#"
            UPDATE accounts
            SET banned_at = now(), ban_reason = $2,
                suspended_at = NULL, suspend_expires_at = NULL, suspend_reason = NULL
            WHERE id = $1
            "#,
        )
        .bind(account_id.as_ref())
        .bind(reason)
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }
}

impl DependOnAccountReadModel for PostgresDatabase {
    type AccountReadModel = PostgresAccountReadModel;

    fn account_read_model(&self) -> &Self::AccountReadModel {
        &PostgresAccountReadModel
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod test;
