use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection, Transaction};
use kernel::interfaces::modify::{AccountModifier, DependOnAccountModifier};
use kernel::interfaces::query::{AccountQuery, DependOnAccountQuery};
use kernel::prelude::entity::{Account, AccountId, AccountName, AuthAccountId, Nanoid};
use kernel::KernelError;
use std::future::Future;

/// Trait for account repository operations (composed from AccountQuery + AccountModifier)
///
/// This trait is automatically implemented for any type that implements both
/// [`DependOnAccountQuery`] and [`DependOnAccountModifier`] via blanket implementation.
///
/// # Architecture
///
/// ```text
/// Application (uses AccountRepository)
///      ↓
/// Adapter (composes AccountQuery + AccountModifier)
///      ↓
/// Kernel (defines traits)
///      ↑
/// Driver (implements concrete datastore)
/// ```
pub trait AccountRepository: Send + Sync {
    type Transaction: Transaction;

    // Query operations
    fn find_by_id(
        &self,
        transaction: &mut Self::Transaction,
        id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<Option<Account>, KernelError>> + Send;

    fn find_by_auth_id(
        &self,
        transaction: &mut Self::Transaction,
        auth_id: &AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<Vec<Account>, KernelError>> + Send;

    fn find_by_name(
        &self,
        transaction: &mut Self::Transaction,
        name: &AccountName,
    ) -> impl Future<Output = error_stack::Result<Option<Account>, KernelError>> + Send;

    fn find_by_nanoid(
        &self,
        transaction: &mut Self::Transaction,
        nanoid: &Nanoid<Account>,
    ) -> impl Future<Output = error_stack::Result<Option<Account>, KernelError>> + Send;

    // Modifier operations
    fn create(
        &self,
        transaction: &mut Self::Transaction,
        account: &Account,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn update(
        &self,
        transaction: &mut Self::Transaction,
        account: &Account,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn delete(
        &self,
        transaction: &mut Self::Transaction,
        account_id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn link_auth_account(
        &self,
        transaction: &mut Self::Transaction,
        account_id: &AccountId,
        auth_account_id: &AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnAccountRepository: DependOnDatabaseConnection + Send + Sync {
    type AccountRepository: AccountRepository<
        Transaction = <Self::DatabaseConnection as DatabaseConnection>::Transaction,
    >;
    fn account_repository(&self) -> &Self::AccountRepository;
}

// Blanket implementation: any type with AccountQuery + AccountModifier can act as AccountRepository
impl<T> AccountRepository for T
where
    T: DependOnAccountQuery + DependOnAccountModifier + Send + Sync,
{
    type Transaction = <T::AccountQuery as AccountQuery>::Transaction;

    fn find_by_id(
        &self,
        transaction: &mut Self::Transaction,
        id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<Option<Account>, KernelError>> + Send {
        self.account_query().find_by_id(transaction, id)
    }

    fn find_by_auth_id(
        &self,
        transaction: &mut Self::Transaction,
        auth_id: &AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<Vec<Account>, KernelError>> + Send {
        self.account_query().find_by_auth_id(transaction, auth_id)
    }

    fn find_by_name(
        &self,
        transaction: &mut Self::Transaction,
        name: &AccountName,
    ) -> impl Future<Output = error_stack::Result<Option<Account>, KernelError>> + Send {
        self.account_query().find_by_name(transaction, name)
    }

    fn find_by_nanoid(
        &self,
        transaction: &mut Self::Transaction,
        nanoid: &Nanoid<Account>,
    ) -> impl Future<Output = error_stack::Result<Option<Account>, KernelError>> + Send {
        self.account_query().find_by_nanoid(transaction, nanoid)
    }

    fn create(
        &self,
        transaction: &mut Self::Transaction,
        account: &Account,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        self.account_modifier().create(transaction, account)
    }

    fn update(
        &self,
        transaction: &mut Self::Transaction,
        account: &Account,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        self.account_modifier().update(transaction, account)
    }

    fn delete(
        &self,
        transaction: &mut Self::Transaction,
        account_id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        self.account_modifier().delete(transaction, account_id)
    }

    fn link_auth_account(
        &self,
        transaction: &mut Self::Transaction,
        account_id: &AccountId,
        auth_account_id: &AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        self.account_modifier()
            .link_auth_account(transaction, account_id, auth_account_id)
    }
}

// Blanket implementation: any type that satisfies the requirements provides DependOnAccountRepository
impl<T> DependOnAccountRepository for T
where
    T: DependOnAccountQuery + DependOnAccountModifier + DependOnDatabaseConnection + Send + Sync,
{
    type AccountRepository = Self;
    fn account_repository(&self) -> &Self::AccountRepository {
        self
    }
}
