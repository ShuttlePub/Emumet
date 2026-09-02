use crate::dto::profile::{CreateProfileDto, ProfileDto};
use crate::service::session_context::OrganizationContext;
use error_stack::Report;
use kernel::interfaces::database::{
    DatabaseConnection, DependOnTransactionManager, TransactionManager,
};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::read_model::{
    DependOnProfileQuery, DependOnProfileReadModel, ProfileQuery, ProfileReadModel,
};
use kernel::interfaces::repository::{AggregateRepository, DependOnProfileRepository};
use kernel::prelude::entity::{Nanoid, Profile, ProfileDisplayName, ProfileId, ProfileSummary};
use kernel::KernelError;
use std::future::Future;

pub trait CreateOrganizationProfileUseCase:
    'static
    + Clone
    + DependOnProfileQuery
    + DependOnProfileRepository
    + DependOnProfileReadModel
    + DependOnTransactionManager
{
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

            let mut connection = self.database_connection().connection().await?;
            if self
                .profile_query()
                .find_by_account_id(&mut connection, &org_context.org_account_id)
                .await?
                .is_some()
            {
                return Err(Report::new(KernelError::Rejected)
                    .attach_printable("Organization profile already exists"));
            }

            let account_id = org_context.org_account_id;
            let account_nanoid = org_context.org_account_nanoid;
            let deps = self.clone();
            let profile = self
                .transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        let command = Profile::create(
                            ProfileId::new(kernel::generate_id()),
                            account_id,
                            display_name,
                            summary,
                            None,
                            None,
                            Nanoid::<Profile>::default(),
                        );
                        let event = deps.profile_repository().save(executor, command).await?;
                        let mut profile = None;
                        Profile::apply(&mut profile, event)?;
                        let profile = profile.ok_or_else(|| {
                            Report::new(KernelError::Internal)
                                .attach_printable("Failed to construct organization profile")
                        })?;
                        deps.profile_read_model().create(executor, &profile).await?;
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
        + DependOnProfileQuery
        + DependOnProfileRepository
        + DependOnProfileReadModel
        + DependOnTransactionManager
{
}
