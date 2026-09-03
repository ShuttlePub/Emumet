use super::database::MockConnection;
use kernel::interfaces::read_model::{AccountQuery, AccountReadModel, AccountWarning};
use kernel::prelude::entity::{Account, AccountId, AccountName, AuthAccountId, Nanoid};
use kernel::KernelError;
use time::OffsetDateTime;

#[derive(Clone)]
pub struct MockAccountQuery {
    pub owned_accounts: Vec<Account>,
    pub target: Option<Account>,
}

impl AccountQuery for MockAccountQuery {
    type Connection = MockConnection;

    async fn find_by_id(
        &self,
        _executor: &mut Self::Connection,
        id: &AccountId,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        Ok(self
            .target
            .as_ref()
            .filter(|account| account.id() == id)
            .cloned())
    }

    async fn find_by_auth_id(
        &self,
        _executor: &mut Self::Connection,
        _auth_id: &AuthAccountId,
    ) -> error_stack::Result<Vec<Account>, KernelError> {
        Ok(self.owned_accounts.clone())
    }

    async fn find_auth_account_id_by_account_id(
        &self,
        _executor: &mut Self::Connection,
        _account_id: &AccountId,
    ) -> error_stack::Result<Option<AuthAccountId>, KernelError> {
        Ok(None)
    }

    async fn find_by_name(
        &self,
        _executor: &mut Self::Connection,
        name: &AccountName,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        Ok(self
            .target
            .as_ref()
            .filter(|account| account.name() == name)
            .cloned())
    }

    async fn find_by_nanoid(
        &self,
        _executor: &mut Self::Connection,
        nanoid: &Nanoid<Account>,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        Ok(self
            .target
            .as_ref()
            .filter(|account| account.nanoid() == nanoid)
            .cloned())
    }

    async fn find_by_nanoids(
        &self,
        _executor: &mut Self::Connection,
        nanoids: &[Nanoid<Account>],
    ) -> error_stack::Result<Vec<Account>, KernelError> {
        Ok(self
            .target
            .as_ref()
            .filter(|account| nanoids.contains(account.nanoid()))
            .cloned()
            .into_iter()
            .collect())
    }

    async fn find_by_id_unfiltered(
        &self,
        executor: &mut Self::Connection,
        id: &AccountId,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        AccountQuery::find_by_id(self, executor, id).await
    }

    async fn find_by_nanoid_unfiltered(
        &self,
        executor: &mut Self::Connection,
        nanoid: &Nanoid<Account>,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        AccountQuery::find_by_nanoid(self, executor, nanoid).await
    }

    async fn find_by_nanoids_unfiltered(
        &self,
        executor: &mut Self::Connection,
        nanoids: &[Nanoid<Account>],
    ) -> error_stack::Result<Vec<Account>, KernelError> {
        AccountQuery::find_by_nanoids(self, executor, nanoids).await
    }

    async fn find_by_nanoid_including_deleted(
        &self,
        executor: &mut Self::Connection,
        nanoid: &Nanoid<Account>,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        AccountQuery::find_by_nanoid(self, executor, nanoid).await
    }

    async fn is_linked_including_deleted(
        &self,
        _executor: &mut Self::Connection,
        _auth_id: &AuthAccountId,
        _account_id: &AccountId,
    ) -> error_stack::Result<bool, KernelError> {
        Ok(false)
    }
}

impl AccountReadModel for MockAccountQuery {
    type Connection = MockConnection;

    async fn find_by_id(
        &self,
        executor: &mut Self::Connection,
        id: &AccountId,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        AccountQuery::find_by_id(self, executor, id).await
    }

    async fn find_by_auth_id(
        &self,
        executor: &mut Self::Connection,
        auth_id: &AuthAccountId,
    ) -> error_stack::Result<Vec<Account>, KernelError> {
        AccountQuery::find_by_auth_id(self, executor, auth_id).await
    }

    async fn find_auth_account_id_by_account_id(
        &self,
        executor: &mut Self::Connection,
        account_id: &AccountId,
    ) -> error_stack::Result<Option<AuthAccountId>, KernelError> {
        AccountQuery::find_auth_account_id_by_account_id(self, executor, account_id).await
    }

    async fn find_by_name(
        &self,
        executor: &mut Self::Connection,
        name: &AccountName,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        AccountQuery::find_by_name(self, executor, name).await
    }

    async fn find_by_nanoid(
        &self,
        executor: &mut Self::Connection,
        nanoid: &Nanoid<Account>,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        AccountQuery::find_by_nanoid(self, executor, nanoid).await
    }

    async fn find_by_nanoids(
        &self,
        executor: &mut Self::Connection,
        nanoids: &[Nanoid<Account>],
    ) -> error_stack::Result<Vec<Account>, KernelError> {
        AccountQuery::find_by_nanoids(self, executor, nanoids).await
    }

    async fn find_by_id_unfiltered(
        &self,
        executor: &mut Self::Connection,
        id: &AccountId,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        AccountQuery::find_by_id(self, executor, id).await
    }

    async fn find_by_nanoid_unfiltered(
        &self,
        executor: &mut Self::Connection,
        nanoid: &Nanoid<Account>,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        AccountQuery::find_by_nanoid(self, executor, nanoid).await
    }

    async fn find_by_nanoids_unfiltered(
        &self,
        executor: &mut Self::Connection,
        nanoids: &[Nanoid<Account>],
    ) -> error_stack::Result<Vec<Account>, KernelError> {
        AccountQuery::find_by_nanoids(self, executor, nanoids).await
    }

    async fn find_by_nanoid_including_deleted(
        &self,
        executor: &mut Self::Connection,
        nanoid: &Nanoid<Account>,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        AccountQuery::find_by_nanoid(self, executor, nanoid).await
    }

    async fn is_linked_including_deleted(
        &self,
        _executor: &mut Self::Connection,
        _auth_id: &AuthAccountId,
        _account_id: &AccountId,
    ) -> error_stack::Result<bool, KernelError> {
        Ok(false)
    }

    async fn find_warnings(
        &self,
        _executor: &mut Self::Connection,
        _account_id: &AccountId,
    ) -> error_stack::Result<Vec<AccountWarning>, KernelError> {
        Ok(Vec::new())
    }

    async fn create(
        &self,
        _executor: &mut Self::Connection,
        _account: &Account,
    ) -> error_stack::Result<(), KernelError> {
        Ok(())
    }

    async fn update(
        &self,
        _executor: &mut Self::Connection,
        _account: &Account,
    ) -> error_stack::Result<(), KernelError> {
        Ok(())
    }

    async fn deactivate(
        &self,
        _executor: &mut Self::Connection,
        _account_id: &AccountId,
    ) -> error_stack::Result<(), KernelError> {
        Ok(())
    }

    async fn unlink_all_auth_accounts(
        &self,
        _executor: &mut Self::Connection,
        _account_id: &AccountId,
    ) -> error_stack::Result<(), KernelError> {
        Ok(())
    }

    async fn link_auth_account(
        &self,
        _executor: &mut Self::Connection,
        _account_id: &AccountId,
        _auth_id: &AuthAccountId,
    ) -> error_stack::Result<(), KernelError> {
        Ok(())
    }

    async fn find_by_id_including_deleted(
        &self,
        executor: &mut Self::Connection,
        id: &AccountId,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        AccountQuery::find_by_id_unfiltered(self, executor, id).await
    }

    async fn suspend(
        &self,
        _executor: &mut Self::Connection,
        _account_id: &AccountId,
        _reason: &str,
        _expires_at: Option<OffsetDateTime>,
    ) -> error_stack::Result<(), KernelError> {
        Ok(())
    }

    async fn unsuspend(
        &self,
        _executor: &mut Self::Connection,
        _account_id: &AccountId,
    ) -> error_stack::Result<(), KernelError> {
        Ok(())
    }

    async fn ban(
        &self,
        _executor: &mut Self::Connection,
        _account_id: &AccountId,
        _reason: &str,
    ) -> error_stack::Result<(), KernelError> {
        Ok(())
    }
}
