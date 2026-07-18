use crate::permission::{account_view, check_permission};
use crate::transfer::account::AccountDto;
use crate::transfer::pagination::{apply_pagination, Pagination};
use adapter::processor::account::{AccountQueryProcessor, DependOnAccountQueryProcessor};
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::permission::DependOnPermissionChecker;
use kernel::prelude::entity::{Account, AuthAccountId, Nanoid};
use kernel::KernelError;
use std::future::Future;

pub trait GetAccountUseCase:
    'static + Sync + Send + DependOnAccountQueryProcessor + DependOnPermissionChecker
{
    // find_by_auth_id returns only accounts owned by the authenticated user,
    // so no additional permission check is needed.
    fn get_all_accounts(
        &self,
        auth_account_id: &AuthAccountId,
        Pagination {
            direction,
            cursor,
            limit,
        }: Pagination<String>,
    ) -> impl Future<Output = error_stack::Result<Option<Vec<AccountDto>>, KernelError>> + Send
    {
        async move {
            let mut transaction = self.database_connection().get_executor().await?;
            let accounts = self
                .account_query_processor()
                .find_by_auth_id(&mut transaction, auth_account_id)
                .await?;
            let cursor = if let Some(cursor) = cursor {
                let id: Nanoid<Account> = Nanoid::new(cursor);
                self.account_query_processor()
                    .find_by_nanoid(&mut transaction, &id)
                    .await?
            } else {
                None
            };
            let accounts = apply_pagination(accounts, limit, cursor, direction);
            Ok(Some(accounts.into_iter().map(AccountDto::from).collect()))
        }
    }

    fn get_accounts_by_ids(
        &self,
        auth_account_id: &AuthAccountId,
        ids: Vec<String>,
    ) -> impl Future<Output = error_stack::Result<Vec<AccountDto>, KernelError>> + Send {
        async move {
            let mut transaction = self.database_connection().get_executor().await?;

            let nanoids: Vec<Nanoid<Account>> =
                ids.into_iter().map(Nanoid::<Account>::new).collect();
            let accounts = self
                .account_query_processor()
                .find_by_nanoids(&mut transaction, &nanoids)
                .await?;

            let mut result = Vec::new();
            for account in accounts {
                if check_permission(self, auth_account_id, &account_view(account.id()))
                    .await
                    .is_ok()
                {
                    result.push(AccountDto::from(account));
                }
            }

            Ok(result)
        }
    }
}

impl<T> GetAccountUseCase for T where
    T: 'static + DependOnAccountQueryProcessor + DependOnPermissionChecker
{
}
