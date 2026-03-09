use crate::service::auth_account::get_auth_account;
use crate::transfer::account::AccountDto;
use crate::transfer::auth_account::AuthAccountInfo;
use crate::transfer::pagination::{apply_pagination, Pagination};
use adapter::crypto::{DependOnSigningKeyGenerator, SigningKeyGenerator};
use error_stack::Report;
use kernel::interfaces::crypto::{DependOnPasswordProvider, PasswordProvider};
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::event_store::{AccountEventStore, DependOnAccountEventStore};
use kernel::interfaces::modify::{
    DependOnAuthAccountModifier, DependOnAuthHostModifier, DependOnEventModifier,
};
use kernel::interfaces::query::{
    DependOnAuthAccountQuery, DependOnAuthHostQuery, DependOnEventQuery,
};
use kernel::interfaces::read_model::{AccountReadModel, DependOnAccountReadModel};
use kernel::interfaces::signal::Signal;
use kernel::prelude::entity::{
    Account, AccountEvent, AccountId, AccountIsBot, AccountName, AccountPrivateKey,
    AccountPublicKey, AuthAccountId, CommandEnvelope, EventId, Nanoid,
};
use kernel::KernelError;
use serde_json;
use std::future::Future;

pub trait GetAccountService:
    'static
    + Sync
    + Send
    + DependOnAccountReadModel
    + DependOnAuthAccountQuery
    + DependOnAuthAccountModifier
    + DependOnAuthHostQuery
    + DependOnAuthHostModifier
    + DependOnEventModifier
    + DependOnEventQuery
{
    fn get_all_accounts(
        &self,
        signal: &impl Signal<AuthAccountId>,
        account: AuthAccountInfo,
        Pagination {
            direction,
            cursor,
            limit,
        }: Pagination<String>,
    ) -> impl Future<Output = error_stack::Result<Option<Vec<AccountDto>>, KernelError>> {
        async move {
            let auth_account = get_auth_account(self, signal, account).await?;
            let mut transaction = self.database_connection().begin_transaction().await?;
            let accounts = self
                .account_read_model()
                .find_by_auth_id(&mut transaction, auth_account.id())
                .await?;
            let cursor = if let Some(cursor) = cursor {
                let id: Nanoid<Account> = Nanoid::new(cursor);
                self.account_read_model()
                    .find_by_nanoid(&mut transaction, &id)
                    .await?
            } else {
                None
            };
            let accounts = apply_pagination(accounts, limit, cursor, direction);
            Ok(Some(accounts.into_iter().map(AccountDto::from).collect()))
        }
    }

    fn get_account_by_id(
        &self,
        signal: &impl Signal<AuthAccountId>,
        auth_info: AuthAccountInfo,
        account_id: String,
    ) -> impl Future<Output = error_stack::Result<AccountDto, KernelError>> {
        async move {
            let auth_account = get_auth_account(self, signal, auth_info).await?;
            let mut transaction = self.database_connection().begin_transaction().await?;

            let nanoid = Nanoid::<Account>::new(account_id);
            let account = self
                .account_read_model()
                .find_by_nanoid(&mut transaction, &nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        nanoid.as_ref()
                    ))
                })?;

            let auth_account_id = auth_account.id();
            let accounts = self
                .account_read_model()
                .find_by_auth_id(&mut transaction, auth_account_id)
                .await?;

            let found = accounts.iter().any(|a| a.id() == account.id());
            if !found {
                return Err(Report::new(KernelError::PermissionDenied)
                    .attach_printable("This account does not belong to the authenticated user"));
            }

            Ok(AccountDto::from(account))
        }
    }
}

impl<T> GetAccountService for T where
    T: 'static
        + DependOnAccountReadModel
        + DependOnAuthAccountQuery
        + DependOnAuthAccountModifier
        + DependOnAuthHostQuery
        + DependOnAuthHostModifier
        + DependOnEventModifier
        + DependOnEventQuery
{
}

pub trait CreateAccountService:
    'static
    + Sync
    + Send
    + DependOnAccountReadModel
    + DependOnAccountEventStore
    + DependOnAuthAccountQuery
    + DependOnAuthAccountModifier
    + DependOnAuthHostQuery
    + DependOnAuthHostModifier
    + DependOnEventModifier
    + DependOnEventQuery
    + DependOnPasswordProvider
    + DependOnSigningKeyGenerator
{
    fn create_account<S>(
        &self,
        signal: &S,
        auth_info: AuthAccountInfo,
        name: String,
        is_bot: bool,
    ) -> impl Future<Output = error_stack::Result<AccountDto, KernelError>>
    where
        S: Signal<AuthAccountId> + Signal<AccountId> + Send + Sync + 'static,
    {
        async move {
            let auth_account = get_auth_account(self, signal, auth_info).await?;
            let mut transaction = self.database_connection().begin_transaction().await?;

            let account_id = AccountId::default();

            // Generate key pair
            let master_password = self.password_provider().get_password()?;
            let key_pair = self.signing_key_generator().generate(&master_password)?;

            let encrypted_private_key_json = serde_json::to_string(&key_pair.encrypted_private_key)
                .map_err(|e| {
                    Report::new(KernelError::Internal)
                        .attach_printable(format!("Failed to serialize encrypted private key: {e}"))
                })?;

            let private_key = AccountPrivateKey::new(encrypted_private_key_json);
            let public_key = AccountPublicKey::new(key_pair.public_key_pem);
            let account_name = AccountName::new(name);
            let account_is_bot = AccountIsBot::new(is_bot);
            let nanoid = Nanoid::<Account>::default();

            // Create command and persist event
            let event = AccountEvent::Created {
                name: account_name,
                private_key,
                public_key,
                is_bot: account_is_bot,
                nanoid,
            };
            let command = CommandEnvelope::new(
                EventId::from(account_id.clone()),
                event.name(),
                event,
                Some(kernel::prelude::entity::KnownEventVersion::Nothing),
            );

            let event_envelope = self
                .account_event_store()
                .persist_and_transform(&mut transaction, command)
                .await?;

            // Apply event to build entity
            let mut account = None;
            Account::apply(&mut account, event_envelope)?;
            let account = account.ok_or_else(|| {
                Report::new(KernelError::Internal)
                    .attach_printable("Failed to construct account from created event")
            })?;

            // Update projection
            self.account_read_model()
                .create(&mut transaction, &account)
                .await?;

            // Link auth account
            self.account_read_model()
                .link_auth_account(&mut transaction, &account_id, auth_account.id())
                .await?;

            Ok(AccountDto::from(account))
        }
    }
}

impl<T> CreateAccountService for T where
    T: 'static
        + DependOnAccountReadModel
        + DependOnAccountEventStore
        + DependOnAuthAccountQuery
        + DependOnAuthAccountModifier
        + DependOnAuthHostQuery
        + DependOnAuthHostModifier
        + DependOnEventModifier
        + DependOnEventQuery
        + DependOnPasswordProvider
        + DependOnSigningKeyGenerator
{
}

pub trait UpdateAccountService:
    'static + Sync + Send + DependOnAccountReadModel + DependOnAccountEventStore
{
    fn update_account(
        &self,
        account_id: AccountId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> {
        async move {
            let mut transaction = self.database_connection().begin_transaction().await?;
            let account = self
                .account_read_model()
                .find_by_id(&mut transaction, &account_id)
                .await?;
            if let Some(account) = account {
                let event_id = EventId::from(account_id.clone());
                let events = self
                    .account_event_store()
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
                    self.account_read_model()
                        .update(&mut transaction, &account)
                        .await?;
                } else {
                    self.account_read_model()
                        .delete(&mut transaction, &account_id)
                        .await?;
                }
                Ok(())
            } else {
                Err(Report::new(KernelError::Internal)
                    .attach_printable(format!("Failed to get target account: {account_id:?}")))
            }
        }
    }
}

impl<T> UpdateAccountService for T where
    T: 'static + DependOnAccountReadModel + DependOnAccountEventStore
{
}

pub trait DeleteAccountService:
    'static
    + Sync
    + Send
    + DependOnAccountReadModel
    + DependOnAccountEventStore
    + DependOnAuthAccountQuery
    + DependOnAuthAccountModifier
    + DependOnAuthHostQuery
    + DependOnAuthHostModifier
    + DependOnEventModifier
    + DependOnEventQuery
{
    fn delete_account<S>(
        &self,
        signal: &S,
        auth_info: AuthAccountInfo,
        account_id: String,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>>
    where
        S: Signal<AuthAccountId> + Signal<AccountId> + Send + Sync + 'static,
    {
        async move {
            let auth_account = get_auth_account(self, signal, auth_info).await?;
            let mut transaction = self.database_connection().begin_transaction().await?;

            let nanoid = Nanoid::<Account>::new(account_id);
            let account = self
                .account_read_model()
                .find_by_nanoid(&mut transaction, &nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        nanoid.as_ref()
                    ))
                })?;

            let auth_account_id = auth_account.id().clone();
            let accounts = self
                .account_read_model()
                .find_by_auth_id(&mut transaction, &auth_account_id)
                .await?;

            let found = accounts.iter().any(|a| a.id() == account.id());
            if !found {
                return Err(Report::new(KernelError::PermissionDenied)
                    .attach_printable("This account does not belong to the authenticated user"));
            }

            let account_id = account.id().clone();
            let current_version = account.version().clone();
            let delete_command = Account::delete(account_id.clone(), current_version);

            self.account_event_store()
                .persist_and_transform(&mut transaction, delete_command)
                .await?;
            signal.emit(account_id).await?;

            Ok(())
        }
    }
}

impl<T> DeleteAccountService for T where
    T: 'static
        + DependOnAccountReadModel
        + DependOnAccountEventStore
        + DependOnAuthAccountQuery
        + DependOnAuthAccountModifier
        + DependOnAuthHostQuery
        + DependOnAuthHostModifier
        + DependOnEventModifier
        + DependOnEventQuery
        + UpdateAccountService
{
}

pub trait EditAccountService:
    'static
    + Sync
    + Send
    + DependOnAccountReadModel
    + DependOnAccountEventStore
    + DependOnAuthAccountQuery
    + DependOnAuthAccountModifier
    + DependOnAuthHostQuery
    + DependOnAuthHostModifier
    + DependOnEventModifier
    + DependOnEventQuery
{
    fn edit_account<S>(
        &self,
        signal: &S,
        auth_info: AuthAccountInfo,
        account_id: String,
        is_bot: bool,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>>
    where
        S: Signal<AuthAccountId> + Signal<AccountId> + Send + Sync + 'static,
    {
        async move {
            let auth_account = get_auth_account(self, signal, auth_info).await?;
            let mut transaction = self.database_connection().begin_transaction().await?;

            let nanoid = Nanoid::<Account>::new(account_id);
            let account = self
                .account_read_model()
                .find_by_nanoid(&mut transaction, &nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        nanoid.as_ref()
                    ))
                })?;

            let auth_account_id = auth_account.id().clone();
            let accounts = self
                .account_read_model()
                .find_by_auth_id(&mut transaction, &auth_account_id)
                .await?;

            let found = accounts.iter().any(|a| a.id() == account.id());
            if !found {
                return Err(Report::new(KernelError::PermissionDenied)
                    .attach_printable("This account does not belong to the authenticated user"));
            }

            let account_id = account.id().clone();
            let current_version = account.version().clone();
            let update_command = Account::update(
                account_id.clone(),
                AccountIsBot::new(is_bot),
                current_version,
            );

            self.account_event_store()
                .persist_and_transform(&mut transaction, update_command)
                .await?;
            signal.emit(account_id).await?;

            Ok(())
        }
    }
}

impl<T> EditAccountService for T where
    T: 'static
        + DependOnAccountReadModel
        + DependOnAccountEventStore
        + DependOnAuthAccountQuery
        + DependOnAuthAccountModifier
        + DependOnAuthHostQuery
        + DependOnAuthHostModifier
        + DependOnEventModifier
        + DependOnEventQuery
        + UpdateAccountService
{
}
