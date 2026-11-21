use crate::dto::profile::{CreateProfileDto, ProfileDto};
use crate::profile_signing_key::CreateProfileSigningKeyUseCase;
use crate::service::session_context::OrganizationContext;
use error_stack::Report;
use kernel::interfaces::config::DependOnPublicBaseUrl;
use kernel::interfaces::crypto::{
    DependOnPasswordProvider, DependOnSigningKeyGenerator, SigningAlgorithm,
};
use kernel::interfaces::database::{DependOnTransactionManager, TransactionManager};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::home_link::{DependOnHomeLinkStore, HomeLinkStore};
use kernel::interfaces::read_model::{DependOnProfileReadModel, ProfileReadModel};
use kernel::interfaces::repository::{
    AggregateRepository, DependOnProfileRepository, DependOnProfileSigningKeyRepository,
};
use kernel::prelude::entity::{
    Nanoid, Profile, ProfileDisplayName, ProfileId, ProfileSummary, ProfileUsername,
};
use kernel::KernelError;
use std::future::Future;

pub trait CreateOrganizationProfileUseCase:
    'static
    + Clone
    + DependOnProfileRepository
    + DependOnProfileReadModel
    + DependOnTransactionManager
    + DependOnProfileSigningKeyRepository
    + DependOnSigningKeyGenerator
    + DependOnPublicBaseUrl
    + DependOnPasswordProvider
    + DependOnHomeLinkStore
{
    /// Creates an organization-owned Profile. `username` is required (P4) and validated before
    /// any side effect; the Profile's actor signing key is stored in the same transaction.
    ///
    /// The Account's Profile set only changes under that Account's lifecycle gate, so the gate is
    /// the first thing the transaction takes. The key pair is generated *before* the transaction:
    /// RSA generation must not consume the gate-holding transaction's bounded time budget.
    fn create_organization_profile(
        &self,
        org_context: OrganizationContext,
        dto: CreateProfileDto,
    ) -> impl Future<Output = error_stack::Result<ProfileDto, KernelError>> + Send + '_ {
        async move {
            let display_name = dto.display_name.map(ProfileDisplayName::new);
            if let Some(display_name) = &display_name {
                display_name.validate()?;
            }
            let summary = dto.summary.map(ProfileSummary::new);
            if let Some(summary) = &summary {
                summary.validate()?;
            }
            let username = Some(ProfileUsername::parse(dto.username)?);

            let account_id = org_context.org_account_id;
            let account_nanoid = org_context.org_account_nanoid;
            let deps = self.clone();
            let key_pair = self.generate_profile_key_pair()?;
            let profile = self
                .transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        deps.home_link_store()
                            .lock_accounts(executor, std::slice::from_ref(&account_id))
                            .await?;
                        let command = Profile::create_with_username(
                            ProfileId::new(kernel::generate_id()),
                            account_id,
                            display_name,
                            summary,
                            None,
                            None,
                            Nanoid::<Profile>::default(),
                            username,
                        );
                        let event = deps.profile_repository().save(executor, command).await?;
                        let mut profile = None;
                        Profile::apply(&mut profile, event)?;
                        let profile = profile.ok_or_else(|| {
                            Report::new(KernelError::Internal)
                                .attach_printable("Failed to construct organization profile")
                        })?;
                        deps.profile_read_model().create(executor, &profile).await?;
                        deps.store_profile_signing_key(
                            executor,
                            profile.id().clone(),
                            profile.nanoid(),
                            SigningAlgorithm::Rsa2048,
                            key_pair,
                        )
                        .await?;
                        Ok(profile)
                    })
                })
                .await?;
            Ok(ProfileDto::new(profile.into(), account_nanoid, None, None))
        }
    }
}

impl<T> CreateOrganizationProfileUseCase for T where
    T: 'static
        + Clone
        + DependOnProfileRepository
        + DependOnProfileReadModel
        + DependOnTransactionManager
        + DependOnProfileSigningKeyRepository
        + DependOnSigningKeyGenerator
        + DependOnPublicBaseUrl
        + DependOnPasswordProvider
        + DependOnHomeLinkStore
{
}
