use error_stack::Report;
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection, Executor};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::event_store::{AuthAccountEventStore, DependOnAuthAccountEventStore};
use kernel::interfaces::read_model::{AuthAccountReadModel, DependOnAuthAccountReadModel};
use kernel::interfaces::signal::Signal;
use kernel::prelude::entity::{AuthAccount, AuthAccountClientId, AuthAccountId, AuthHostId};
use kernel::KernelError;
use std::future::Future;

// --- Signal DI trait (adapter-specific) ---

pub trait DependOnAuthAccountSignal: Send + Sync {
    type AuthAccountSignal: Signal<AuthAccountId> + Send + Sync + 'static;
    fn auth_account_signal(&self) -> &Self::AuthAccountSignal;
}

// --- AuthAccountCommandProcessor ---

pub trait AuthAccountCommandProcessor: Send + Sync + 'static {
    type Executor: Executor;

    fn create(
        &self,
        executor: &mut Self::Executor,
        host: AuthHostId,
        client_id: AuthAccountClientId,
    ) -> impl Future<Output = error_stack::Result<AuthAccount, KernelError>> + Send;

    fn delete(
        &self,
        executor: &mut Self::Executor,
        auth_account_id: AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

impl<T> AuthAccountCommandProcessor for T
where
    T: DependOnAuthAccountEventStore
        + DependOnAuthAccountReadModel
        + DependOnAuthAccountSignal
        + Send
        + Sync
        + 'static,
{
    type Executor = <<T as DependOnAuthAccountEventStore>::AuthAccountEventStore as AuthAccountEventStore>::Executor;

    async fn create(
        &self,
        executor: &mut Self::Executor,
        host: AuthHostId,
        client_id: AuthAccountClientId,
    ) -> error_stack::Result<AuthAccount, KernelError> {
        let auth_account_id = AuthAccountId::default();
        let command = AuthAccount::create(auth_account_id.clone(), host, client_id);

        let event_envelope = self
            .auth_account_event_store()
            .persist_and_transform(executor, command)
            .await?;

        let mut auth_account = None;
        AuthAccount::apply(&mut auth_account, event_envelope)?;
        let auth_account = auth_account.ok_or_else(|| {
            Report::new(KernelError::Internal)
                .attach_printable("Failed to construct auth account from created event")
        })?;

        self.auth_account_read_model()
            .create(executor, &auth_account)
            .await?;

        if let Err(e) = self.auth_account_signal().emit(auth_account_id).await {
            tracing::warn!("Failed to emit auth account signal: {:?}", e);
        }

        Ok(auth_account)
    }

    async fn delete(
        &self,
        executor: &mut Self::Executor,
        auth_account_id: AuthAccountId,
    ) -> error_stack::Result<(), KernelError> {
        let command = AuthAccount::delete(auth_account_id.clone());

        self.auth_account_event_store()
            .persist_and_transform(executor, command)
            .await?;

        self.auth_account_read_model()
            .delete(executor, &auth_account_id)
            .await?;

        if let Err(e) = self.auth_account_signal().emit(auth_account_id).await {
            tracing::warn!("Failed to emit auth account signal: {:?}", e);
        }

        Ok(())
    }
}

pub trait DependOnAuthAccountCommandProcessor: DependOnDatabaseConnection + Send + Sync {
    type AuthAccountCommandProcessor: AuthAccountCommandProcessor<
        Executor = <<Self as DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Executor,
    >;
    fn auth_account_command_processor(&self) -> &Self::AuthAccountCommandProcessor;
}

impl<T> DependOnAuthAccountCommandProcessor for T
where
    T: DependOnAuthAccountEventStore
        + DependOnAuthAccountReadModel
        + DependOnAuthAccountSignal
        + DependOnDatabaseConnection
        + Send
        + Sync
        + 'static,
{
    type AuthAccountCommandProcessor = Self;
    fn auth_account_command_processor(&self) -> &Self::AuthAccountCommandProcessor {
        self
    }
}

// --- AuthAccountQueryProcessor ---

pub trait AuthAccountQueryProcessor: Send + Sync + 'static {
    type Executor: Executor;

    fn find_by_id(
        &self,
        executor: &mut Self::Executor,
        id: &AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<Option<AuthAccount>, KernelError>> + Send;

    fn find_by_client_id(
        &self,
        executor: &mut Self::Executor,
        client_id: &AuthAccountClientId,
    ) -> impl Future<Output = error_stack::Result<Option<AuthAccount>, KernelError>> + Send;
}

impl<T> AuthAccountQueryProcessor for T
where
    T: DependOnAuthAccountReadModel + Send + Sync + 'static,
{
    type Executor =
        <<T as DependOnAuthAccountReadModel>::AuthAccountReadModel as AuthAccountReadModel>::Executor;

    async fn find_by_id(
        &self,
        executor: &mut Self::Executor,
        id: &AuthAccountId,
    ) -> error_stack::Result<Option<AuthAccount>, KernelError> {
        self.auth_account_read_model()
            .find_by_id(executor, id)
            .await
    }

    async fn find_by_client_id(
        &self,
        executor: &mut Self::Executor,
        client_id: &AuthAccountClientId,
    ) -> error_stack::Result<Option<AuthAccount>, KernelError> {
        self.auth_account_read_model()
            .find_by_client_id(executor, client_id)
            .await
    }
}

pub trait DependOnAuthAccountQueryProcessor: DependOnDatabaseConnection + Send + Sync {
    type AuthAccountQueryProcessor: AuthAccountQueryProcessor<
        Executor = <<Self as DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Executor,
    >;
    fn auth_account_query_processor(&self) -> &Self::AuthAccountQueryProcessor;
}

impl<T> DependOnAuthAccountQueryProcessor for T
where
    T: DependOnAuthAccountReadModel + DependOnDatabaseConnection + Send + Sync + 'static,
{
    type AuthAccountQueryProcessor = Self;
    fn auth_account_query_processor(&self) -> &Self::AuthAccountQueryProcessor {
        self
    }
}
