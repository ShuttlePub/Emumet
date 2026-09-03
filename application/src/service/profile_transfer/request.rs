use super::{require_active_membership, resolve_personal_actor_account};
use crate::dto::profile_transfer::ProfileTransferRequestDto;
use error_stack::Report;
use kernel::interfaces::database::{
    DatabaseConnection, DependOnDatabaseConnection, TransactionManager,
};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::read_model::{
    AccountQuery, DependOnAccountQuery, DependOnProfileQuery, DependOnProfileTransferRequestQuery,
    DependOnProfileTransferRequestReadModel, ProfileQuery, ProfileTransferRequestQuery,
    ProfileTransferRequestReadModel,
};
use kernel::interfaces::repository::{
    AggregateRepository, DependOnOrganizationMembershipRepository,
    DependOnProfileTransferRequestRepository,
};
use kernel::prelude::entity::{
    Account, AccountKind, AuthAccountId, Nanoid, Profile, ProfileTransferRequest,
    ProfileTransferRequestId,
};
use kernel::KernelError;
use std::future::Future;

pub trait RequestProfileTransferUseCase: 'static + Sync + Send + Clone {
    fn request_profile_transfer<'a>(
        &'a self,
        auth_id: &'a AuthAccountId,
        profile_nanoid: String,
        org_account_nanoid: String,
    ) -> impl Future<Output = error_stack::Result<ProfileTransferRequestDto, KernelError>> + Send + 'a;
}

impl<T> RequestProfileTransferUseCase for T
where
    T: 'static
        + Sync
        + Send
        + Clone
        + DependOnAccountQuery
        + DependOnProfileQuery
        + DependOnProfileTransferRequestQuery
        + DependOnProfileTransferRequestReadModel
        + DependOnProfileTransferRequestRepository
        + DependOnOrganizationMembershipRepository
        + DependOnDatabaseConnection
        + kernel::interfaces::database::DependOnTransactionManager,
{
    fn request_profile_transfer<'a>(
        &'a self,
        auth_id: &'a AuthAccountId,
        profile_nanoid: String,
        org_account_nanoid: String,
    ) -> impl Future<Output = error_stack::Result<ProfileTransferRequestDto, KernelError>> + Send + 'a
    {
        async move {
            let mut connection = self.database_connection().connection().await?;
            let profile_nanoid = Nanoid::<Profile>::new(profile_nanoid);
            let profile = self
                .profile_query()
                .find_by_nanoid(&mut connection, &profile_nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound)
                        .attach_printable(format!("Profile not found: {}", profile_nanoid.as_ref()))
                })?;
            let org_account = {
                let org_nanoid = Nanoid::<Account>::new(org_account_nanoid);
                self.account_query()
                    .find_by_nanoid(&mut connection, &org_nanoid)
                    .await?
                    .filter(|account| account.kind() == &AccountKind::Organization)
                    .ok_or_else(|| {
                        Report::new(KernelError::NotFound).attach_printable(format!(
                            "Organization account not found: {}",
                            org_nanoid.as_ref()
                        ))
                    })?
            };

            let actor_account = resolve_personal_actor_account(self, auth_id).await?;
            if actor_account.id() != profile.account_id() {
                return Err(Report::new(KernelError::PermissionDenied)
                    .attach_printable("Authenticated user does not own the profile"));
            }
            require_active_membership(self, org_account.id(), actor_account.id()).await?;

            if self
                .profile_transfer_request_query()
                .find_pending_by_profile_id(&mut connection, profile.id())
                .await?
                .is_some()
            {
                return Err(Report::new(KernelError::Rejected)
                    .attach_printable("A pending profile transfer request already exists"));
            }

            let profile_id = profile.id().clone();
            let from_account_id = profile.account_id().clone();
            let to_org_account_id = org_account.id().clone();
            let profile_nanoid = profile.nanoid().as_ref().to_string();
            let org_account_nanoid = org_account.nanoid().as_ref().to_string();
            let deps = self.clone();
            let request = self
                .transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        let command = ProfileTransferRequest::request(
                            ProfileTransferRequestId::new(kernel::generate_id()),
                            profile_id,
                            from_account_id,
                            to_org_account_id,
                            Nanoid::default(),
                        );
                        let event = deps
                            .profile_transfer_request_repository()
                            .save(executor, command)
                            .await?;
                        let mut request = None;
                        ProfileTransferRequest::apply(&mut request, event)?;
                        let request = request.ok_or_else(|| {
                            Report::new(KernelError::Internal)
                                .attach_printable("Failed to construct profile transfer request")
                        })?;
                        deps.profile_transfer_request_read_model()
                            .create(executor, &request)
                            .await?;
                        Ok(request)
                    })
                })
                .await?;
            Ok(ProfileTransferRequestDto::new(
                request.into(),
                profile_nanoid,
                org_account_nanoid,
                "pending".to_string(),
            ))
        }
    }
}
