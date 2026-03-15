use error_stack::Report;
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection, Executor};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::event_store::{AccountEventStore, DependOnAccountEventStore};
use kernel::interfaces::read_model::{AccountReadModel, DependOnAccountReadModel};
use kernel::interfaces::signal::Signal;
use kernel::prelude::entity::{
    Account, AccountId, AccountIsBot, AccountName, AccountPrivateKey, AccountPublicKey,
    AuthAccountId, Nanoid,
};
use kernel::KernelError;
use std::future::Future;
use time::OffsetDateTime;

#[derive(Debug)]
pub struct CreateAccountParam {
    pub name: AccountName,
    pub private_key: AccountPrivateKey,
    pub public_key: AccountPublicKey,
    pub is_bot: AccountIsBot,
    pub auth_account_id: AuthAccountId,
}

#[derive(Debug)]
pub struct UpdateAccountParam {
    pub account_id: AccountId,
    pub is_bot: AccountIsBot,
    pub current_version: kernel::prelude::entity::EventVersion<Account>,
}

// --- Signal DI trait (adapter-specific) ---

pub trait DependOnAccountSignal: Send + Sync {
    type AccountSignal: Signal<AccountId> + Send + Sync + 'static;
    fn account_signal(&self) -> &Self::AccountSignal;
}

// --- AccountCommandProcessor ---

pub trait AccountCommandProcessor: Send + Sync + 'static {
    type Executor: Executor;

    fn create(
        &self,
        executor: &mut Self::Executor,
        param: CreateAccountParam,
    ) -> impl Future<Output = error_stack::Result<Account, KernelError>> + Send;

    fn update(
        &self,
        executor: &mut Self::Executor,
        param: UpdateAccountParam,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn deactivate(
        &self,
        executor: &mut Self::Executor,
        account_id: AccountId,
        current_version: kernel::prelude::entity::EventVersion<Account>,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn suspend(
        &self,
        executor: &mut Self::Executor,
        account_id: AccountId,
        reason: String,
        expires_at: Option<OffsetDateTime>,
        current_version: kernel::prelude::entity::EventVersion<Account>,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn unsuspend(
        &self,
        executor: &mut Self::Executor,
        account_id: AccountId,
        current_version: kernel::prelude::entity::EventVersion<Account>,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn ban(
        &self,
        executor: &mut Self::Executor,
        account_id: AccountId,
        reason: String,
        current_version: kernel::prelude::entity::EventVersion<Account>,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

impl<T> AccountCommandProcessor for T
where
    T: DependOnAccountEventStore + DependOnAccountSignal + Send + Sync + 'static,
{
    type Executor =
        <<T as DependOnAccountEventStore>::AccountEventStore as AccountEventStore>::Executor;

    async fn create(
        &self,
        executor: &mut Self::Executor,
        param: CreateAccountParam,
    ) -> error_stack::Result<Account, KernelError> {
        let CreateAccountParam {
            name,
            private_key,
            public_key,
            is_bot,
            auth_account_id,
        } = param;
        let account_id = AccountId::default();
        let nanoid = Nanoid::<Account>::default();
        let command = Account::create(
            account_id.clone(),
            name,
            private_key,
            public_key,
            is_bot,
            nanoid,
            auth_account_id,
        );

        let event_envelope = self
            .account_event_store()
            .persist_and_transform(executor, command)
            .await?;

        let mut account = None;
        Account::apply(&mut account, event_envelope)?;
        let account = account.ok_or_else(|| {
            Report::new(KernelError::Internal)
                .attach_printable("Failed to construct account from created event")
        })?;

        if let Err(e) = self.account_signal().emit(account_id).await {
            tracing::warn!("Failed to emit account signal: {:?}", e);
        }

        Ok(account)
    }

    async fn update(
        &self,
        executor: &mut Self::Executor,
        param: UpdateAccountParam,
    ) -> error_stack::Result<(), KernelError> {
        let command = Account::update(
            param.account_id.clone(),
            param.is_bot,
            param.current_version,
        );

        self.account_event_store()
            .persist_and_transform(executor, command)
            .await?;

        if let Err(e) = self.account_signal().emit(param.account_id).await {
            tracing::warn!("Failed to emit account signal: {:?}", e);
        }

        Ok(())
    }

    async fn deactivate(
        &self,
        executor: &mut Self::Executor,
        account_id: AccountId,
        current_version: kernel::prelude::entity::EventVersion<Account>,
    ) -> error_stack::Result<(), KernelError> {
        let command = Account::deactivate(account_id.clone(), current_version);

        self.account_event_store()
            .persist_and_transform(executor, command)
            .await?;

        if let Err(e) = self.account_signal().emit(account_id).await {
            tracing::warn!("Failed to emit account signal: {:?}", e);
        }

        Ok(())
    }

    async fn suspend(
        &self,
        executor: &mut Self::Executor,
        account_id: AccountId,
        reason: String,
        expires_at: Option<OffsetDateTime>,
        current_version: kernel::prelude::entity::EventVersion<Account>,
    ) -> error_stack::Result<(), KernelError> {
        let command = Account::suspend(account_id.clone(), reason, expires_at, current_version);

        self.account_event_store()
            .persist_and_transform(executor, command)
            .await?;

        if let Err(e) = self.account_signal().emit(account_id).await {
            tracing::warn!("Failed to emit account signal: {:?}", e);
        }

        Ok(())
    }

    async fn unsuspend(
        &self,
        executor: &mut Self::Executor,
        account_id: AccountId,
        current_version: kernel::prelude::entity::EventVersion<Account>,
    ) -> error_stack::Result<(), KernelError> {
        let command = Account::unsuspend(account_id.clone(), current_version);

        self.account_event_store()
            .persist_and_transform(executor, command)
            .await?;

        if let Err(e) = self.account_signal().emit(account_id).await {
            tracing::warn!("Failed to emit account signal: {:?}", e);
        }

        Ok(())
    }

    async fn ban(
        &self,
        executor: &mut Self::Executor,
        account_id: AccountId,
        reason: String,
        current_version: kernel::prelude::entity::EventVersion<Account>,
    ) -> error_stack::Result<(), KernelError> {
        let command = Account::ban(account_id.clone(), reason, current_version);

        self.account_event_store()
            .persist_and_transform(executor, command)
            .await?;

        if let Err(e) = self.account_signal().emit(account_id).await {
            tracing::warn!("Failed to emit account signal: {:?}", e);
        }

        Ok(())
    }
}

pub trait DependOnAccountCommandProcessor: DependOnDatabaseConnection + Send + Sync {
    type AccountCommandProcessor: AccountCommandProcessor<
        Executor = <<Self as DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Executor,
    >;
    fn account_command_processor(&self) -> &Self::AccountCommandProcessor;
}

impl<T> DependOnAccountCommandProcessor for T
where
    T: DependOnAccountEventStore
        + DependOnAccountSignal
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
