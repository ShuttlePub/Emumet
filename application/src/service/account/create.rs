use crate::signing_key::CreateSigningKeyUseCase;
use crate::transfer::account::{AccountDto, CreateAccountDto};
use adapter::crypto::DependOnSigningKeyGenerator;
use adapter::processor::account::{
    AccountCommandProcessor, CreateAccountParam, DependOnAccountCommandProcessor,
};
use adapter::processor::profile::{
    CreateProfileParam, DependOnProfileCommandProcessor, ProfileCommandProcessor,
};
use kernel::interfaces::config::DependOnPublicBaseUrl;
use kernel::interfaces::crypto::{DependOnPasswordProvider, SigningAlgorithm};
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::permission::{
    AccountRelation, DependOnPermissionWriter, PermissionWriter, RelationTarget,
};
use kernel::interfaces::repository::DependOnSigningKeyRepository;
use kernel::prelude::entity::{
    AccountIsBot, AccountName, AuthAccountId, Nanoid, Profile, ProfileDisplayName,
};
use kernel::KernelError;
use std::future::Future;

pub trait CreateAccountUseCase:
    'static
    + Sync
    + Send
    + DependOnAccountCommandProcessor
    + DependOnProfileCommandProcessor
    + DependOnPasswordProvider
    + DependOnSigningKeyGenerator
    + DependOnPermissionWriter
    + DependOnSigningKeyRepository
    + DependOnPublicBaseUrl
{
    fn create_account(
        &self,
        auth_account_id: AuthAccountId,
        dto: CreateAccountDto,
    ) -> impl Future<Output = error_stack::Result<AccountDto, KernelError>> + Send {
        async move {
            let mut transaction = self.database_connection().get_executor().await?;

            let account_name = AccountName::new(dto.name);
            let account_is_bot = AccountIsBot::new(dto.is_bot);

            let display_name = ProfileDisplayName::new(account_name.as_ref().to_string());

            let account = self
                .account_command_processor()
                .create(
                    &mut transaction,
                    CreateAccountParam {
                        name: account_name,
                        is_bot: account_is_bot,
                        auth_account_id: auth_account_id.clone(),
                    },
                )
                .await?;

            self.profile_command_processor()
                .create(
                    &mut transaction,
                    CreateProfileParam {
                        account_id: account.id().clone(),
                        display_name: Some(display_name),
                        summary: None,
                        icon: None,
                        banner: None,
                        nano_id: Nanoid::<Profile>::default(),
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

            self.create(
                account.id().clone(),
                account.nanoid(),
                SigningAlgorithm::Rsa2048,
            )
            .await?;

            Ok(AccountDto::from(account))
        }
    }
}

impl<T> CreateAccountUseCase for T where
    T: 'static
        + DependOnAccountCommandProcessor
        + DependOnProfileCommandProcessor
        + DependOnPasswordProvider
        + DependOnSigningKeyGenerator
        + DependOnPermissionWriter
        + DependOnSigningKeyRepository
        + DependOnPublicBaseUrl
{
}
