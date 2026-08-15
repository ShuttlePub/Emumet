use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::{Account, AccountId, AccountName, AuthAccountId, Nanoid};
use crate::KernelError;
use std::future::Future;
use time::OffsetDateTime;

pub trait AccountReadModel: Sync + Send + 'static {
    type Connection: Connection;

    // Query operations (projection reads)
    fn find_by_id(
        &self,
        executor: &mut Self::Connection,
        id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<Option<Account>, KernelError>> + Send;

    fn find_by_auth_id(
        &self,
        executor: &mut Self::Connection,
        auth_id: &AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<Vec<Account>, KernelError>> + Send;

    fn find_by_name(
        &self,
        executor: &mut Self::Connection,
        name: &AccountName,
    ) -> impl Future<Output = error_stack::Result<Option<Account>, KernelError>> + Send;

    fn find_by_nanoid(
        &self,
        executor: &mut Self::Connection,
        nanoid: &Nanoid<Account>,
    ) -> impl Future<Output = error_stack::Result<Option<Account>, KernelError>> + Send;

    fn find_by_nanoids(
        &self,
        executor: &mut Self::Connection,
        nanoids: &[Nanoid<Account>],
    ) -> impl Future<Output = error_stack::Result<Vec<Account>, KernelError>> + Send;

    // Projection update operations (called by EventApplier pipeline)
    fn create(
        &self,
        executor: &mut Self::Connection,
        account: &Account,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn update(
        &self,
        executor: &mut Self::Connection,
        account: &Account,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn deactivate(
        &self,
        executor: &mut Self::Connection,
        account_id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn unlink_all_auth_accounts(
        &self,
        executor: &mut Self::Connection,
        account_id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn link_auth_account(
        &self,
        executor: &mut Self::Connection,
        account_id: &AccountId,
        auth_account_id: &AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    // Unfiltered queries (include suspended/banned, exclude only deleted)
    fn find_by_id_unfiltered(
        &self,
        executor: &mut Self::Connection,
        id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<Option<Account>, KernelError>> + Send;

    fn find_by_nanoid_unfiltered(
        &self,
        executor: &mut Self::Connection,
        nanoid: &Nanoid<Account>,
    ) -> impl Future<Output = error_stack::Result<Option<Account>, KernelError>> + Send;

    fn find_by_nanoids_unfiltered(
        &self,
        executor: &mut Self::Connection,
        nanoids: &[Nanoid<Account>],
    ) -> impl Future<Output = error_stack::Result<Vec<Account>, KernelError>> + Send;

    /// Load an account including deleted rows.  Used by child projectors
    /// to detect cascade-deleted parents without weakening the corruption
    /// detection that `find_by_id` / `find_by_id_unfiltered` provides.
    fn find_by_id_including_deleted(
        &self,
        executor: &mut Self::Connection,
        id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<Option<Account>, KernelError>> + Send;

    // Moderation operations
    fn suspend(
        &self,
        executor: &mut Self::Connection,
        account_id: &AccountId,
        reason: &str,
        expires_at: Option<OffsetDateTime>,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn unsuspend(
        &self,
        executor: &mut Self::Connection,
        account_id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn ban(
        &self,
        executor: &mut Self::Connection,
        account_id: &AccountId,
        reason: &str,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnAccountReadModel: Sync + Send + DependOnDatabaseConnection {
    type AccountReadModel: AccountReadModel<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn account_read_model(&self) -> &Self::AccountReadModel;
}

/// Read-model query facade for accounts (ADR 0006 decision 3: `*Query` 系統).
///
/// Blanket-implemented for any [`DependOnAccountReadModel`]; obtain it via
/// [`DependOnAccountQuery::account_query`].
pub trait AccountQuery: Send + Sync + 'static {
    type Connection: Connection;

    fn find_by_id(
        &self,
        executor: &mut Self::Connection,
        id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<Option<Account>, KernelError>> + Send;

    fn find_by_auth_id(
        &self,
        executor: &mut Self::Connection,
        auth_id: &AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<Vec<Account>, KernelError>> + Send;

    fn find_by_name(
        &self,
        executor: &mut Self::Connection,
        name: &AccountName,
    ) -> impl Future<Output = error_stack::Result<Option<Account>, KernelError>> + Send;

    fn find_by_nanoid(
        &self,
        executor: &mut Self::Connection,
        nanoid: &Nanoid<Account>,
    ) -> impl Future<Output = error_stack::Result<Option<Account>, KernelError>> + Send;

    fn find_by_nanoids(
        &self,
        executor: &mut Self::Connection,
        nanoids: &[Nanoid<Account>],
    ) -> impl Future<Output = error_stack::Result<Vec<Account>, KernelError>> + Send;

    fn find_by_id_unfiltered(
        &self,
        executor: &mut Self::Connection,
        id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<Option<Account>, KernelError>> + Send;

    fn find_by_nanoid_unfiltered(
        &self,
        executor: &mut Self::Connection,
        nanoid: &Nanoid<Account>,
    ) -> impl Future<Output = error_stack::Result<Option<Account>, KernelError>> + Send;

    fn find_by_nanoids_unfiltered(
        &self,
        executor: &mut Self::Connection,
        nanoids: &[Nanoid<Account>],
    ) -> impl Future<Output = error_stack::Result<Vec<Account>, KernelError>> + Send;
}

impl<T> AccountQuery for T
where
    T: DependOnAccountReadModel + Send + Sync + 'static,
{
    type Connection =
        <<T as DependOnAccountReadModel>::AccountReadModel as AccountReadModel>::Connection;

    async fn find_by_id(
        &self,
        executor: &mut Self::Connection,
        id: &AccountId,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        self.account_read_model().find_by_id(executor, id).await
    }

    async fn find_by_auth_id(
        &self,
        executor: &mut Self::Connection,
        auth_id: &AuthAccountId,
    ) -> error_stack::Result<Vec<Account>, KernelError> {
        self.account_read_model()
            .find_by_auth_id(executor, auth_id)
            .await
    }

    async fn find_by_name(
        &self,
        executor: &mut Self::Connection,
        name: &AccountName,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        self.account_read_model().find_by_name(executor, name).await
    }

    async fn find_by_nanoid(
        &self,
        executor: &mut Self::Connection,
        nanoid: &Nanoid<Account>,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        self.account_read_model()
            .find_by_nanoid(executor, nanoid)
            .await
    }

    async fn find_by_nanoids(
        &self,
        executor: &mut Self::Connection,
        nanoids: &[Nanoid<Account>],
    ) -> error_stack::Result<Vec<Account>, KernelError> {
        self.account_read_model()
            .find_by_nanoids(executor, nanoids)
            .await
    }

    async fn find_by_id_unfiltered(
        &self,
        executor: &mut Self::Connection,
        id: &AccountId,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        self.account_read_model()
            .find_by_id_unfiltered(executor, id)
            .await
    }

    async fn find_by_nanoid_unfiltered(
        &self,
        executor: &mut Self::Connection,
        nanoid: &Nanoid<Account>,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        self.account_read_model()
            .find_by_nanoid_unfiltered(executor, nanoid)
            .await
    }

    async fn find_by_nanoids_unfiltered(
        &self,
        executor: &mut Self::Connection,
        nanoids: &[Nanoid<Account>],
    ) -> error_stack::Result<Vec<Account>, KernelError> {
        self.account_read_model()
            .find_by_nanoids_unfiltered(executor, nanoids)
            .await
    }
}

pub trait DependOnAccountQuery: DependOnDatabaseConnection + Send + Sync {
    type AccountQuery: AccountQuery<
        Connection = <<Self as DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Connection,
    >;
    fn account_query(&self) -> &Self::AccountQuery;
}

impl<T> DependOnAccountQuery for T
where
    T: DependOnAccountReadModel + DependOnDatabaseConnection + Send + Sync + 'static,
{
    type AccountQuery = Self;
    fn account_query(&self) -> &Self::AccountQuery {
        self
    }
}
