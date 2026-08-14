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
use kernel::interfaces::database::{DependOnTransactionManager, TransactionManager};
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
    + Clone
    + DependOnAccountCommandProcessor
    + DependOnProfileCommandProcessor
    + DependOnPasswordProvider
    + DependOnSigningKeyGenerator
    + DependOnPermissionWriter
    + DependOnSigningKeyRepository
    + DependOnPublicBaseUrl
    + DependOnTransactionManager
{
    fn create_account(
        &self,
        auth_account_id: AuthAccountId,
        dto: CreateAccountDto,
    ) -> impl Future<Output = error_stack::Result<AccountDto, KernelError>> + Send + '_ {
        async move {
            let account_name = AccountName::new(dto.name);
            let account_is_bot = AccountIsBot::new(dto.is_bot);
            let display_name = ProfileDisplayName::new(account_name.as_ref().to_string());
            let transaction_auth_account_id = auth_account_id.clone();
            let deps = self.clone();
            let account = self
                .transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        let account = deps
                            .account_command_processor()
                            .create(
                                executor,
                                CreateAccountParam {
                                    name: account_name,
                                    is_bot: account_is_bot,
                                    auth_account_id: transaction_auth_account_id,
                                },
                            )
                            .await?;

                        deps.profile_command_processor()
                            .create(
                                executor,
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

                        deps.create(
                            executor,
                            account.id().clone(),
                            account.nanoid(),
                            SigningAlgorithm::Rsa2048,
                        )
                        .await?;

                        Ok(account)
                    })
                })
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
        + Clone
        + DependOnAccountCommandProcessor
        + DependOnProfileCommandProcessor
        + DependOnPasswordProvider
        + DependOnSigningKeyGenerator
        + DependOnPermissionWriter
        + DependOnSigningKeyRepository
        + DependOnPublicBaseUrl
        + DependOnTransactionManager
{
}
