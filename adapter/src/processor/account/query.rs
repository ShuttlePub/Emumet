use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection, Executor};
use kernel::interfaces::read_model::{AccountReadModel, DependOnAccountReadModel};
use kernel::prelude::entity::{Account, AccountId, AccountName, AuthAccountId, Nanoid};
use kernel::KernelError;
use std::future::Future;

// --- AccountQueryProcessor ---

pub trait AccountQueryProcessor: Send + Sync + 'static {
    type Executor: Executor;

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
}

impl<T> AccountQueryProcessor for T
where
    T: DependOnAccountReadModel + Send + Sync + 'static,
{
    type Executor =
        <<T as DependOnAccountReadModel>::AccountReadModel as AccountReadModel>::Executor;

    async fn find_by_id(
        &self,
        executor: &mut Self::Executor,
        id: &AccountId,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        self.account_read_model().find_by_id(executor, id).await
    }

    async fn find_by_auth_id(
        &self,
        executor: &mut Self::Executor,
        auth_id: &AuthAccountId,
    ) -> error_stack::Result<Vec<Account>, KernelError> {
        self.account_read_model()
            .find_by_auth_id(executor, auth_id)
            .await
    }

    async fn find_by_name(
        &self,
        executor: &mut Self::Executor,
        name: &AccountName,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        self.account_read_model().find_by_name(executor, name).await
    }

    async fn find_by_nanoid(
        &self,
        executor: &mut Self::Executor,
        nanoid: &Nanoid<Account>,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        self.account_read_model()
            .find_by_nanoid(executor, nanoid)
            .await
    }

    async fn find_by_nanoids(
        &self,
        executor: &mut Self::Executor,
        nanoids: &[Nanoid<Account>],
    ) -> error_stack::Result<Vec<Account>, KernelError> {
        self.account_read_model()
            .find_by_nanoids(executor, nanoids)
            .await
    }

    async fn find_by_id_unfiltered(
        &self,
        executor: &mut Self::Executor,
        id: &AccountId,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        self.account_read_model()
            .find_by_id_unfiltered(executor, id)
            .await
    }

    async fn find_by_nanoid_unfiltered(
        &self,
        executor: &mut Self::Executor,
        nanoid: &Nanoid<Account>,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        self.account_read_model()
            .find_by_nanoid_unfiltered(executor, nanoid)
            .await
    }

    async fn find_by_nanoids_unfiltered(
        &self,
        executor: &mut Self::Executor,
        nanoids: &[Nanoid<Account>],
    ) -> error_stack::Result<Vec<Account>, KernelError> {
        self.account_read_model()
            .find_by_nanoids_unfiltered(executor, nanoids)
            .await
    }
}

pub trait DependOnAccountQueryProcessor: DependOnDatabaseConnection + Send + Sync {
    type AccountQueryProcessor: AccountQueryProcessor<
        Executor = <<Self as DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Executor,
    >;
    fn account_query_processor(&self) -> &Self::AccountQueryProcessor;
}

impl<T> DependOnAccountQueryProcessor for T
where
    T: DependOnAccountReadModel + DependOnDatabaseConnection + Send + Sync + 'static,
{
    type AccountQueryProcessor = Self;
    fn account_query_processor(&self) -> &Self::AccountQueryProcessor {
        self
    }
}
