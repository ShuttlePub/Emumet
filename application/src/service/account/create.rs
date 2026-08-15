use crate::dto::account::{AccountDto, CreateAccountDto};
use crate::signing_key::CreateSigningKeyUseCase;
use error_stack::Report;
use kernel::interfaces::config::DependOnPublicBaseUrl;
use kernel::interfaces::crypto::{
    DependOnPasswordProvider, DependOnSigningKeyGenerator, SigningAlgorithm,
};
use kernel::interfaces::database::{DependOnTransactionManager, TransactionManager};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::permission::{
    AccountRelation, DependOnPermissionWriter, PermissionWriter, RelationTarget,
};
use kernel::interfaces::read_model::{
    AccountReadModel, DependOnAccountReadModel, DependOnProfileReadModel, ProfileReadModel,
};
use kernel::interfaces::repository::{
    AggregateRepository, DependOnAccountRepository, DependOnProfileRepository,
    DependOnSigningKeyRepository,
};
use kernel::prelude::entity::{
    Account, AccountId, AccountIsBot, AccountName, AuthAccountId, Nanoid, Profile,
    ProfileDisplayName, ProfileId,
};
use kernel::KernelError;
use std::future::Future;

pub trait CreateAccountUseCase:
    'static
    + Sync
    + Send
    + Clone
    + DependOnAccountRepository
    + DependOnAccountReadModel
    + DependOnProfileRepository
    + DependOnProfileReadModel
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
                        let account_id = AccountId::default();
                        let command = Account::create(
                            account_id.clone(),
                            account_name,
                            account_is_bot,
                            Nanoid::<Account>::default(),
                            transaction_auth_account_id.clone(),
                        );

                        let event_envelope =
                            deps.account_repository().save(executor, command).await?;

                        let mut account = None;
                        Account::apply(&mut account, event_envelope)?;
                        let account = account.ok_or_else(|| {
                            Report::new(KernelError::Internal)
                                .attach_printable("Failed to construct account from created event")
                        })?;

                        if let Err(e) = deps.account_read_model().create(executor, &account).await {
                            tracing::error!(?e, "Failed to create account read model");
                            return Err(e);
                        }

                        if let Err(e) = deps
                            .account_read_model()
                            .link_auth_account(executor, &account_id, &transaction_auth_account_id)
                            .await
                        {
                            tracing::error!(?e, "Failed to link auth account");
                            return Err(e);
                        }

                        let profile_command = Profile::create(
                            ProfileId::new(kernel::generate_id()),
                            account.id().clone(),
                            Some(display_name),
                            None,
                            None,
                            None,
                            Nanoid::<Profile>::default(),
                        );

                        let event_envelope = deps
                            .profile_repository()
                            .save(executor, profile_command)
                            .await?;

                        let mut profile = None;
                        Profile::apply(&mut profile, event_envelope)?;
                        let profile = profile.ok_or_else(|| {
                            Report::new(KernelError::Internal)
                                .attach_printable("Failed to construct profile from created event")
                        })?;

                        if let Err(e) = deps.profile_read_model().create(executor, &profile).await {
                            tracing::error!(?e, "Failed to create profile read model");
                            return Err(e);
                        }

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
        + DependOnAccountRepository
        + DependOnAccountReadModel
        + DependOnProfileRepository
        + DependOnProfileReadModel
        + DependOnPasswordProvider
        + DependOnSigningKeyGenerator
        + DependOnPermissionWriter
        + DependOnSigningKeyRepository
        + DependOnPublicBaseUrl
        + DependOnTransactionManager
{
}
