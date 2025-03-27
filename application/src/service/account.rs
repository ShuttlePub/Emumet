use crate::service::auth_account::get_auth_account;
use crate::transfer::account::AccountDto;
use crate::transfer::auth_account::AuthAccountInfo;
use crate::transfer::pagination::{apply_pagination, Pagination};
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::modify::{
    AccountModifier, DependOnAccountModifier, DependOnAuthAccountModifier,
    DependOnAuthHostModifier, DependOnEventModifier,
};
use kernel::interfaces::query::{
    AccountQuery, DependOnAccountQuery, DependOnAuthAccountQuery, DependOnAuthHostQuery,
    DependOnEventQuery, EventQuery,
};
use kernel::prelude::entity::{Account, AccountId, EventId, Nanoid};
use kernel::KernelError;
use std::future::Future;

pub trait GetAccountService:
    'static
    + Sync
    + Send
    + DependOnAccountQuery
    + DependOnAuthAccountQuery
    + DependOnAuthAccountModifier
    + DependOnAuthHostQuery
    + DependOnAuthHostModifier
    + DependOnEventModifier
    + DependOnEventQuery
{
    fn get_all_accounts(
        &self,
        account: AuthAccountInfo,
        Pagination {
            direction,
            cursor,
            limit,
        }: Pagination<String>,
    ) -> impl Future<Output = error_stack::Result<Option<Vec<AccountDto>>, KernelError>> {
        async move {
            let auth_account = get_auth_account(self, account).await?;
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
            Ok(Some(accounts.into_iter().map(AccountDto::from).collect()))
        }
    }
}

impl<T> GetAccountService for T where
    T: 'static
        + DependOnAccountQuery
        + DependOnAuthAccountQuery
        + DependOnAuthAccountModifier
        + DependOnAuthHostQuery
        + DependOnAuthHostModifier
        + DependOnEventModifier
        + DependOnEventQuery
{
}

pub trait UpdateAccountService:
    'static
    + DependOnAccountQuery
    + DependOnAccountModifier
    + DependOnEventQuery
    + DependOnEventModifier
{
    fn update_account(
        &self,
        account_id: AccountId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> {
        async move {
            let mut transaction = self.database_connection().begin_transaction().await?;
            let account = self
                .account_query()
                .find_by_id(&mut transaction, &account_id)
                .await?;
            if let Some(account) = account {
                let event_id = EventId::from(account_id.clone());
                let events = self
                    .event_query()
                    .find_by_id(&mut transaction, &event_id, Some(account.version()))
                    .await?;
                if events.is_empty() {
                    return Ok(());
                }
                let mut account = Some(account);
                for event in events {
                    Account::apply(&mut account, event)?;
                }
                if let Some(account) = account {
                    self.account_modifier()
                        .update(&mut transaction, &account)
                        .await?;
                } else {
                    self.account_modifier()
                        .delete(&mut transaction, &account_id)
                        .await?;
                }
                Ok(())
            } else {
                Err(error_stack::Report::new(KernelError::Internal)
                    .attach_printable(format!("Failed to get target account: {account_id:?}")))
            }
        }
    }
}

impl<T> UpdateAccountService for T where
    T: 'static
        + DependOnAccountQuery
        + DependOnAccountModifier
        + DependOnEventQuery
        + DependOnEventModifier
{
}
