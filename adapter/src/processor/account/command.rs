use error_stack::Report;
use kernel::interfaces::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::read_model::{AccountReadModel, DependOnAccountReadModel};
use kernel::interfaces::repository::{AggregateRepository, DependOnAccountRepository};
use kernel::prelude::entity::{
    Account, AccountId, AccountIsBot, AccountName, AuthAccountId, EventVersion, Nanoid,
};
use kernel::KernelError;
use std::future::Future;
use time::OffsetDateTime;

#[derive(Debug)]
pub struct CreateAccountParam {
    pub name: AccountName,
    pub is_bot: AccountIsBot,
    pub auth_account_id: AuthAccountId,
}

#[derive(Debug)]
pub struct UpdateAccountParam {
    pub account_id: AccountId,
    pub is_bot: AccountIsBot,
    pub current_version: EventVersion<Account>,
}

// --- AccountCommandProcessor ---

pub trait AccountCommandProcessor: Send + Sync + 'static {
    type Connection: Connection;

    fn create(
        &self,
        executor: &mut Self::Connection,
        param: CreateAccountParam,
    ) -> impl Future<Output = error_stack::Result<Account, KernelError>> + Send;

    fn update(
        &self,
        executor: &mut Self::Connection,
        param: UpdateAccountParam,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn deactivate(
        &self,
        executor: &mut Self::Connection,
        account_id: AccountId,
        current_version: kernel::prelude::entity::EventVersion<Account>,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn suspend(
        &self,
        executor: &mut Self::Connection,
        account_id: AccountId,
        reason: String,
        expires_at: Option<OffsetDateTime>,
        current_version: kernel::prelude::entity::EventVersion<Account>,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn unsuspend(
        &self,
        executor: &mut Self::Connection,
        account_id: AccountId,
        current_version: kernel::prelude::entity::EventVersion<Account>,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn ban(
        &self,
        executor: &mut Self::Connection,
        account_id: AccountId,
        reason: String,
        current_version: kernel::prelude::entity::EventVersion<Account>,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

impl<T> AccountCommandProcessor for T
where
    T: DependOnAccountRepository + DependOnAccountReadModel + Send + Sync + 'static,
{
    type Connection =
        <<T as DependOnAccountRepository>::AccountRepository as AggregateRepository<Account>>::Connection;

    async fn create(
        &self,
        executor: &mut Self::Connection,
        param: CreateAccountParam,
    ) -> error_stack::Result<Account, KernelError> {
        let CreateAccountParam {
            name,
            is_bot,
            auth_account_id,
        } = param;
        let account_id = AccountId::default();
        let nanoid = Nanoid::<Account>::default();
        let command = Account::create(
            account_id.clone(),
            name,
            is_bot,
            nanoid,
            auth_account_id.clone(),
        );

        let event_envelope = self.account_repository().save(executor, command).await?;

        let mut account = None;
        Account::apply(&mut account, event_envelope)?;
        let account = account.ok_or_else(|| {
            Report::new(KernelError::Internal)
                .attach_printable("Failed to construct account from created event")
        })?;

        if let Err(e) = self.account_read_model().create(executor, &account).await {
            tracing::error!(?e, "Failed to create account read model");
            return Err(e);
        }

        if let Err(e) = self
            .account_read_model()
            .link_auth_account(executor, &account_id, &auth_account_id)
            .await
        {
            tracing::error!(?e, "Failed to link auth account");
            return Err(e);
        }

        Ok(account)
    }

    async fn update(
        &self,
        executor: &mut Self::Connection,
        param: UpdateAccountParam,
    ) -> error_stack::Result<(), KernelError> {
        let command = Account::update(
            param.account_id.clone(),
            param.is_bot,
            param.current_version,
        );

        self.account_repository().save(executor, command).await?;

        Ok(())
    }

    async fn deactivate(
        &self,
        executor: &mut Self::Connection,
        account_id: AccountId,
        current_version: kernel::prelude::entity::EventVersion<Account>,
    ) -> error_stack::Result<(), KernelError> {
        let command = Account::deactivate(account_id.clone(), current_version);

        self.account_repository().save(executor, command).await?;

        Ok(())
    }

    async fn suspend(
        &self,
        executor: &mut Self::Connection,
        account_id: AccountId,
        reason: String,
        expires_at: Option<OffsetDateTime>,
        current_version: kernel::prelude::entity::EventVersion<Account>,
    ) -> error_stack::Result<(), KernelError> {
        let command = Account::suspend(account_id.clone(), reason, expires_at, current_version);

        self.account_repository().save(executor, command).await?;

        Ok(())
    }

    async fn unsuspend(
        &self,
        executor: &mut Self::Connection,
        account_id: AccountId,
        current_version: kernel::prelude::entity::EventVersion<Account>,
    ) -> error_stack::Result<(), KernelError> {
        let command = Account::unsuspend(account_id.clone(), current_version);

        self.account_repository().save(executor, command).await?;

        Ok(())
    }

    async fn ban(
        &self,
        executor: &mut Self::Connection,
        account_id: AccountId,
        reason: String,
        current_version: kernel::prelude::entity::EventVersion<Account>,
    ) -> error_stack::Result<(), KernelError> {
        let command = Account::ban(account_id.clone(), reason, current_version);

        self.account_repository().save(executor, command).await?;

        Ok(())
    }
}

pub trait DependOnAccountCommandProcessor: DependOnDatabaseConnection + Send + Sync {
    type AccountCommandProcessor: AccountCommandProcessor<
        Connection = <<Self as DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Connection,
    >;
    fn account_command_processor(&self) -> &Self::AccountCommandProcessor;
}

impl<T> DependOnAccountCommandProcessor for T
where
    T: DependOnAccountRepository
        + DependOnAccountReadModel
        + DependOnDatabaseConnection
        + Send
        + Sync
        + 'static,
{
    type AccountCommandProcessor = Self;
    fn account_command_processor(&self) -> &Self::AccountCommandProcessor {
        self
    }
}
