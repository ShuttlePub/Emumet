use crate::service::auth_account::get_auth_account;
use crate::transfer::account::AccountDto;
use crate::transfer::pagination::{apply_pagination, Pagination};
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::query::{AccountQuery, DependOnAccountQuery, DependOnAuthAccountQuery};
use kernel::prelude::entity::{Account, Nanoid};
use kernel::KernelError;
use std::future::Future;

pub trait GetAccountService:
    'static + Sync + Send + DependOnAccountQuery + DependOnAuthAccountQuery
{
    fn get_all_accounts(
        &self,
        subject: String,
        Pagination {
            direction,
            cursor,
            limit,
        }: Pagination<String>,
    ) -> impl Future<Output = error_stack::Result<Vec<AccountDto>, KernelError>> {
        async move {
            let auth_account = if let Some(auth_account) = get_auth_account(self, subject).await? {
                auth_account
            } else {
                return Ok(vec![]);
            };
            let mut transaction = self.database_connection().begin_transaction().await?;
            let accounts = self
                .account_query()
                .find_by_auth_id(&mut transaction, auth_account.id())
                .await?;
            let cursor = if let Some(cursor) = cursor {
                let id: Nanoid<Account> = Nanoid::new(cursor);
                self.account_query()
                    .find_by_nanoid(&mut transaction, &id)
                    .await?
            } else {
                None
            };
            let accounts = apply_pagination(accounts, limit, cursor, direction);
            Ok(accounts.into_iter().map(AccountDto::from).collect())
        }
    }
}

impl<T> GetAccountService for T where T: 'static + DependOnAccountQuery + DependOnAuthAccountQuery {}
