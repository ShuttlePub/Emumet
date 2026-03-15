use crate::permission::{
    account_deactivate, account_edit, account_view, check_permission, instance_moderate,
};
use crate::transfer::account::{AccountDto, CreateAccountDto, UpdateAccountDto};
use crate::transfer::pagination::{apply_pagination, Pagination};
use adapter::crypto::{DependOnSigningKeyGenerator, SigningKeyGenerator};
use adapter::processor::account::{
    AccountCommandProcessor, AccountQueryProcessor, CreateAccountParam,
    DependOnAccountCommandProcessor, DependOnAccountQueryProcessor, UpdateAccountParam,
};
use error_stack::Report;
use kernel::interfaces::crypto::{DependOnPasswordProvider, PasswordProvider};
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::permission::{
    AccountRelation, DependOnPermissionChecker, DependOnPermissionWriter, PermissionWriter,
    RelationTarget,
};
use kernel::prelude::entity::{
    Account, AccountIsBot, AccountName, AccountPrivateKey, AccountPublicKey, AuthAccountId, Nanoid,
};
use kernel::KernelError;
use serde_json;
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

pub trait CreateAccountUseCase:
    'static
    + Sync
    + Send
    + DependOnAccountCommandProcessor
    + DependOnPasswordProvider
    + DependOnSigningKeyGenerator
    + DependOnPermissionWriter
{
    fn create_account(
        &self,
        auth_account_id: AuthAccountId,
        dto: CreateAccountDto,
    ) -> impl Future<Output = error_stack::Result<AccountDto, KernelError>> + Send {
        async move {
            let mut transaction = self.database_connection().get_executor().await?;

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
            let account_name = AccountName::new(dto.name);
            let account_is_bot = AccountIsBot::new(dto.is_bot);

            let account = self
                .account_command_processor()
                .create(
                    &mut transaction,
                    CreateAccountParam {
                        name: account_name,
                        private_key,
                        public_key,
                        is_bot: account_is_bot,
                        auth_account_id: auth_account_id.clone(),
                    },
                )
                .await?;

            self.permission_writer()
                .create_relation(
                    &RelationTarget::Account {
                        account_id: account.id().clone(),
                        relation: AccountRelation::Owner,
                    },
                    &auth_account_id,
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
        + DependOnPermissionWriter
{
}

pub trait UpdateAccountUseCase:
    'static
    + Sync
    + Send
    + DependOnAccountCommandProcessor
    + DependOnAccountQueryProcessor
    + DependOnPermissionChecker
{
    fn update_account(
        &self,
        auth_account_id: &AuthAccountId,
        dto: UpdateAccountDto,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        async move {
            let mut transaction = self.database_connection().get_executor().await?;

            let nanoid = Nanoid::<Account>::new(dto.account_nanoid);
            let account = self
                .account_query_processor()
                .find_by_nanoid_unfiltered(&mut transaction, &nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        nanoid.as_ref()
                    ))
                })?;

            check_permission(self, auth_account_id, &account_edit(account.id())).await?;

            if !account.status().is_active() {
                return Err(Report::new(KernelError::Rejected)
                    .attach_printable("Cannot modify a suspended or banned account"));
            }

            self.account_command_processor()
                .update(
                    &mut transaction,
                    UpdateAccountParam {
                        account_id: account.id().clone(),
                        is_bot: AccountIsBot::new(dto.is_bot),
                        current_version: account.version().clone(),
                    },
                )
                .await?;

            Ok(())
        }
    }
}

impl<T> UpdateAccountUseCase for T where
    T: 'static
        + DependOnAccountCommandProcessor
        + DependOnAccountQueryProcessor
        + DependOnPermissionChecker
{
}

pub trait DeactivateAccountUseCase:
    'static
    + Sync
    + Send
    + DependOnAccountCommandProcessor
    + DependOnAccountQueryProcessor
    + DependOnPermissionChecker
    + DependOnPermissionWriter
{
    fn deactivate_account(
        &self,
        auth_account_id: &AuthAccountId,
        account_id: String,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        async move {
            let mut transaction = self.database_connection().get_executor().await?;

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

            check_permission(self, auth_account_id, &account_deactivate(account.id())).await?;

            let account_id = account.id().clone();
            let current_version = account.version().clone();
            self.account_command_processor()
                .deactivate(&mut transaction, account_id.clone(), current_version)
                .await?;

            self.permission_writer()
                .delete_relation(
                    &RelationTarget::Account {
                        account_id,
                        relation: AccountRelation::Owner,
                    },
                    auth_account_id,
                )
                .await?;

            Ok(())
        }
    }
}

impl<T> DeactivateAccountUseCase for T where
    T: 'static
        + DependOnAccountCommandProcessor
        + DependOnAccountQueryProcessor
        + DependOnPermissionChecker
        + DependOnPermissionWriter
{
}

pub trait SuspendAccountUseCase:
    'static
    + Sync
    + Send
    + DependOnAccountCommandProcessor
    + DependOnAccountQueryProcessor
    + DependOnPermissionChecker
{
    fn suspend_account(
        &self,
        auth_account_id: &AuthAccountId,
        account_id: String,
        reason: String,
        expires_at: Option<time::OffsetDateTime>,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        async move {
            let mut transaction = self.database_connection().get_executor().await?;

            let nanoid = Nanoid::<Account>::new(account_id);
            let account = self
                .account_query_processor()
                .find_by_nanoid_unfiltered(&mut transaction, &nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        nanoid.as_ref()
                    ))
                })?;

            check_permission(self, auth_account_id, &instance_moderate()).await?;

            if let Some(exp) = expires_at {
                if exp <= time::OffsetDateTime::now_utc() {
                    return Err(Report::new(KernelError::Rejected)
                        .attach_printable("expires_at must be in the future"));
                }
            }

            if !account.status().is_active() {
                return Err(
                    Report::new(KernelError::Rejected).attach_printable("Account is not active")
                );
            }
            if account.deleted_at().is_some() {
                return Err(
                    Report::new(KernelError::Rejected).attach_printable("Account is deactivated")
                );
            }

            let account_id = account.id().clone();
            let current_version = account.version().clone();
            self.account_command_processor()
                .suspend(
                    &mut transaction,
                    account_id,
                    reason,
                    expires_at,
                    current_version,
                )
                .await?;

            Ok(())
        }
    }
}

impl<T> SuspendAccountUseCase for T where
    T: 'static
        + DependOnAccountCommandProcessor
        + DependOnAccountQueryProcessor
        + DependOnPermissionChecker
{
}

pub trait UnsuspendAccountUseCase:
    'static
    + Sync
    + Send
    + DependOnAccountCommandProcessor
    + DependOnAccountQueryProcessor
    + DependOnPermissionChecker
{
    fn unsuspend_account(
        &self,
        auth_account_id: &AuthAccountId,
        account_id: String,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        async move {
            let mut transaction = self.database_connection().get_executor().await?;

            let nanoid = Nanoid::<Account>::new(account_id);
            let account = self
                .account_query_processor()
                .find_by_nanoid_unfiltered(&mut transaction, &nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        nanoid.as_ref()
                    ))
                })?;

            check_permission(self, auth_account_id, &instance_moderate()).await?;

            if !account.status().is_suspended() {
                return Err(
                    Report::new(KernelError::Rejected).attach_printable("Account is not suspended")
                );
            }

            let account_id = account.id().clone();
            let current_version = account.version().clone();
            self.account_command_processor()
                .unsuspend(&mut transaction, account_id, current_version)
                .await?;

            Ok(())
        }
    }
}

impl<T> UnsuspendAccountUseCase for T where
    T: 'static
        + DependOnAccountCommandProcessor
        + DependOnAccountQueryProcessor
        + DependOnPermissionChecker
{
}

pub trait BanAccountUseCase:
    'static
    + Sync
    + Send
    + DependOnAccountCommandProcessor
    + DependOnAccountQueryProcessor
    + DependOnPermissionChecker
{
    fn ban_account(
        &self,
        auth_account_id: &AuthAccountId,
        account_id: String,
        reason: String,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        async move {
            let mut transaction = self.database_connection().get_executor().await?;

            let nanoid = Nanoid::<Account>::new(account_id);
            let account = self
                .account_query_processor()
                .find_by_nanoid_unfiltered(&mut transaction, &nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        nanoid.as_ref()
                    ))
                })?;

            check_permission(self, auth_account_id, &instance_moderate()).await?;

            if account.status().is_banned() {
                return Err(Report::new(KernelError::Rejected)
                    .attach_printable("Account is already banned"));
            }
            if account.deleted_at().is_some() {
                return Err(
                    Report::new(KernelError::Rejected).attach_printable("Account is deactivated")
                );
            }

            let account_id = account.id().clone();
            let current_version = account.version().clone();
            self.account_command_processor()
                .ban(&mut transaction, account_id, reason, current_version)
                .await?;

            Ok(())
        }
    }
}

impl<T> BanAccountUseCase for T where
    T: 'static
        + DependOnAccountCommandProcessor
        + DependOnAccountQueryProcessor
        + DependOnPermissionChecker
{
}
