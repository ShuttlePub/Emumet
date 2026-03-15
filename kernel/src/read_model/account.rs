use crate::database::{DatabaseConnection, DependOnDatabaseConnection, Executor};
use crate::entity::{Account, AccountId, AccountName, AuthAccountId, Nanoid};
use crate::KernelError;
use std::future::Future;
use time::OffsetDateTime;

pub trait AccountReadModel: Sync + Send + 'static {
    type Executor: Executor;

    // Query operations (projection reads)
    fn find_by_id(
        &self,
        executor: &mut Self::Executor,
        id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<Option<Account>, KernelError>> + Send;

    fn find_by_auth_id(
        &self,
        executor: &mut Self::Executor,
        auth_id: &AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<Vec<Account>, KernelError>> + Send;

    fn find_by_name(
        &self,
        executor: &mut Self::Executor,
        name: &AccountName,
    ) -> impl Future<Output = error_stack::Result<Option<Account>, KernelError>> + Send;

    fn find_by_nanoid(
        &self,
        executor: &mut Self::Executor,
        nanoid: &Nanoid<Account>,
    ) -> impl Future<Output = error_stack::Result<Option<Account>, KernelError>> + Send;

    fn find_by_nanoids(
        &self,
        executor: &mut Self::Executor,
        nanoids: &[Nanoid<Account>],
    ) -> impl Future<Output = error_stack::Result<Vec<Account>, KernelError>> + Send;

    // Projection update operations (called by EventApplier pipeline)
    fn create(
        &self,
        executor: &mut Self::Executor,
        account: &Account,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn update(
        &self,
        executor: &mut Self::Executor,
        account: &Account,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn deactivate(
        &self,
        executor: &mut Self::Executor,
        account_id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn unlink_all_auth_accounts(
        &self,
        executor: &mut Self::Executor,
        account_id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn link_auth_account(
        &self,
        executor: &mut Self::Executor,
        account_id: &AccountId,
        auth_account_id: &AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    // Unfiltered queries (include suspended/banned, exclude only deleted)
    fn find_by_id_unfiltered(
        &self,
        executor: &mut Self::Executor,
        id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<Option<Account>, KernelError>> + Send;

    fn find_by_nanoid_unfiltered(
        &self,
        executor: &mut Self::Executor,
        nanoid: &Nanoid<Account>,
    ) -> impl Future<Output = error_stack::Result<Option<Account>, KernelError>> + Send;

    fn find_by_nanoids_unfiltered(
        &self,
        executor: &mut Self::Executor,
        nanoids: &[Nanoid<Account>],
    ) -> impl Future<Output = error_stack::Result<Vec<Account>, KernelError>> + Send;

    // Moderation operations
    fn suspend(
        &self,
        executor: &mut Self::Executor,
        account_id: &AccountId,
        reason: &str,
        expires_at: Option<OffsetDateTime>,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn unsuspend(
        &self,
        executor: &mut Self::Executor,
        account_id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn ban(
        &self,
        executor: &mut Self::Executor,
        account_id: &AccountId,
        reason: &str,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnAccountReadModel: Sync + Send + DependOnDatabaseConnection {
    type AccountReadModel: AccountReadModel<
        Executor = <Self::DatabaseConnection as DatabaseConnection>::Executor,
    >;

    fn account_read_model(&self) -> &Self::AccountReadModel;
}
