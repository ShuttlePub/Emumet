use crate::transfer::account::AccountDto;
use crate::transfer::pagination::{apply_pagination, Pagination};
use adapter::crypto::{DependOnSigningKeyGenerator, SigningKeyGenerator};
use adapter::processor::account::{
    AccountCommandProcessor, AccountQueryProcessor, DependOnAccountCommandProcessor,
    DependOnAccountQueryProcessor,
};
use error_stack::Report;
use kernel::interfaces::crypto::{DependOnPasswordProvider, PasswordProvider};
use kernel::interfaces::database::DatabaseConnection;
use kernel::prelude::entity::{
    Account, AccountIsBot, AccountName, AccountPrivateKey, AccountPublicKey, AuthAccountId, Nanoid,
};
use kernel::KernelError;
use serde_json;
use std::future::Future;

pub trait GetAccountUseCase: 'static + Sync + Send + DependOnAccountQueryProcessor {
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
            let mut transaction = self.database_connection().begin_transaction().await?;
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

    fn get_account_by_id(
        &self,
        auth_account_id: &AuthAccountId,
        account_id: String,
    ) -> impl Future<Output = error_stack::Result<AccountDto, KernelError>> + Send {
        async move {
            let mut transaction = self.database_connection().begin_transaction().await?;

            let nanoid = Nanoid::<Account>::new(account_id);
            let account = self
                .account_query_processor()
                .find_by_nanoid(&mut transaction, &nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        nanoid.as_ref()
                    ))
                })?;

            let accounts = self
                .account_query_processor()
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

impl<T> GetAccountUseCase for T where T: 'static + DependOnAccountQueryProcessor {}

pub trait CreateAccountUseCase:
    'static
    + Sync
    + Send
    + DependOnAccountCommandProcessor
    + DependOnPasswordProvider
    + DependOnSigningKeyGenerator
{
    fn create_account(
        &self,
        auth_account_id: AuthAccountId,
        name: String,
        is_bot: bool,
    ) -> impl Future<Output = error_stack::Result<AccountDto, KernelError>> + Send {
        async move {
            let mut transaction = self.database_connection().begin_transaction().await?;

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

            let account = self
                .account_command_processor()
                .create(
                    &mut transaction,
                    account_name,
                    private_key,
                    public_key,
                    account_is_bot,
                    auth_account_id,
                )
                .await?;

            Ok(AccountDto::from(account))
        }
    }
}

impl<T> CreateAccountUseCase for T where
    T: 'static
        + DependOnAccountCommandProcessor
        + DependOnPasswordProvider
        + DependOnSigningKeyGenerator
{
}

pub trait EditAccountUseCase:
    'static + Sync + Send + DependOnAccountCommandProcessor + DependOnAccountQueryProcessor
{
    fn edit_account(
        &self,
        auth_account_id: &AuthAccountId,
        account_id: String,
        is_bot: bool,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        async move {
            let mut transaction = self.database_connection().begin_transaction().await?;

            let nanoid = Nanoid::<Account>::new(account_id);
            let account = self
                .account_query_processor()
                .find_by_nanoid(&mut transaction, &nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        nanoid.as_ref()
                    ))
                })?;

            let accounts = self
                .account_query_processor()
                .find_by_auth_id(&mut transaction, auth_account_id)
                .await?;

            let found = accounts.iter().any(|a| a.id() == account.id());
            if !found {
                return Err(Report::new(KernelError::PermissionDenied)
                    .attach_printable("This account does not belong to the authenticated user"));
            }

            let account_id = account.id().clone();
            let current_version = account.version().clone();
            self.account_command_processor()
                .update(
                    &mut transaction,
                    account_id,
                    AccountIsBot::new(is_bot),
                    current_version,
                )
                .await?;

            Ok(())
        }
    }
}

impl<T> EditAccountUseCase for T where
    T: 'static + DependOnAccountCommandProcessor + DependOnAccountQueryProcessor
{
}

pub trait DeleteAccountUseCase:
    'static + Sync + Send + DependOnAccountCommandProcessor + DependOnAccountQueryProcessor
{
    fn delete_account(
        &self,
        auth_account_id: &AuthAccountId,
        account_id: String,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        async move {
            let mut transaction = self.database_connection().begin_transaction().await?;

            let nanoid = Nanoid::<Account>::new(account_id);
            let account = self
                .account_query_processor()
                .find_by_nanoid(&mut transaction, &nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        nanoid.as_ref()
                    ))
                })?;

            let accounts = self
                .account_query_processor()
                .find_by_auth_id(&mut transaction, auth_account_id)
                .await?;

            let found = accounts.iter().any(|a| a.id() == account.id());
            if !found {
                return Err(Report::new(KernelError::PermissionDenied)
                    .attach_printable("This account does not belong to the authenticated user"));
            }

            let account_id = account.id().clone();
            let current_version = account.version().clone();
            self.account_command_processor()
                .delete(&mut transaction, account_id, current_version)
                .await?;

            Ok(())
        }
    }
}

impl<T> DeleteAccountUseCase for T where
    T: 'static + DependOnAccountCommandProcessor + DependOnAccountQueryProcessor
{
}
