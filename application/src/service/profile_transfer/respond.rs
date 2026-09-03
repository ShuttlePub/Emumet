use super::resolve_actor_with_active_membership;
use error_stack::Report;
use kernel::interfaces::database::{
    DatabaseConnection, DependOnDatabaseConnection, TransactionManager,
};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::read_model::{
    DependOnAccountQuery, DependOnProfileQuery, DependOnProfileReadModel,
    DependOnProfileTransferRequestQuery, DependOnProfileTransferRequestReadModel, ProfileReadModel,
    ProfileTransferRequestQuery, ProfileTransferRequestReadModel,
};
use kernel::interfaces::repository::{
    AggregateRepository, DependOnOrganizationMembershipRepository, DependOnProfileRepository,
    DependOnProfileTransferRequestRepository,
};
use kernel::interfaces::storage::{
    DependOnProfileMediaCopyGateway, ProfileMediaCopyGateway, ProfileMediaCopyRequest,
};
use kernel::prelude::entity::{AuthAccountId, Nanoid, OrgRole, Profile, ProfileTransferRequest};
use kernel::KernelError;
use std::future::Future;

#[cfg(test)]
mod tests;

pub trait AcceptProfileTransferRequestUseCase: 'static + Sync + Send + Clone {
    fn accept_profile_transfer_request<'a>(
        &'a self,
        auth_id: &'a AuthAccountId,
        request_nanoid: String,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send + 'a;
}

impl<T> AcceptProfileTransferRequestUseCase for T
where
    T: 'static
        + Sync
        + Send
        + Clone
        + DependOnAccountQuery
        + DependOnProfileQuery
        + DependOnProfileReadModel
        + DependOnProfileRepository
        + DependOnProfileTransferRequestQuery
        + DependOnProfileTransferRequestReadModel
        + DependOnProfileTransferRequestRepository
        + DependOnOrganizationMembershipRepository
        + DependOnProfileMediaCopyGateway
        + DependOnDatabaseConnection
        + kernel::interfaces::database::DependOnTransactionManager,
{
    fn accept_profile_transfer_request<'a>(
        &'a self,
        auth_id: &'a AuthAccountId,
        request_nanoid: String,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send + 'a {
        async move {
            let mut connection = self.database_connection().connection().await?;
            let request_nanoid = Nanoid::<ProfileTransferRequest>::new(request_nanoid);
            let projection = self
                .profile_transfer_request_query()
                .find_by_nanoid(&mut connection, &request_nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Profile transfer request not found: {}",
                        request_nanoid.as_ref()
                    ))
                })?;

            let request_id = projection.id().clone();
            let profile_id = projection.profile_id().clone();
            let from_account_id = projection.from_account_id().clone();
            let to_org_account_id = projection.to_org_account_id().clone();

            resolve_actor_with_active_membership(
                self,
                auth_id,
                &to_org_account_id,
                &[OrgRole::Owner, OrgRole::Admin],
            )
            .await?;

            let deps = self.clone();
            let copy_from_account_id = from_account_id.clone();
            let copy_to_account_id = to_org_account_id.clone();
            let transferred_profile = self
                .transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        let (request, current_version) = deps
                            .profile_transfer_request_repository()
                            .load(executor, &request_id)
                            .await?
                            .into_parts();
                        if !request.status().is_pending() {
                            return Err(Report::new(KernelError::Rejected)
                                .attach_printable("Profile transfer request is not pending"));
                        }
                        let event = deps
                            .profile_transfer_request_repository()
                            .save(
                                executor,
                                ProfileTransferRequest::accept(
                                    request_id.clone(),
                                    current_version.clone(),
                                ),
                            )
                            .await?;
                        let mut updated_request = Some(request);
                        ProfileTransferRequest::apply(&mut updated_request, event)?;
                        let updated_request = updated_request.ok_or_else(|| {
                            Report::new(KernelError::Internal)
                                .attach_printable("Failed to construct accepted transfer request")
                        })?;
                        deps.profile_transfer_request_read_model()
                            .update(executor, &updated_request)
                            .await?;

                        let (profile, _profile_version) = deps
                            .profile_repository()
                            .load(executor, &profile_id)
                            .await?
                            .into_parts();
                        if profile.account_id() != &from_account_id {
                            return Err(Report::new(KernelError::Rejected)
                                .attach_printable("profile owner changed"));
                        }
                        let transfer_command = Profile::transfer_account(
                            profile_id.clone(),
                            from_account_id.clone(),
                            to_org_account_id.clone(),
                        );
                        let transfer_event = deps
                            .profile_repository()
                            .save(executor, transfer_command)
                            .await?;
                        let mut transferred_profile = Some(profile);
                        Profile::apply(&mut transferred_profile, transfer_event)?;
                        let transferred_profile = transferred_profile.ok_or_else(|| {
                            Report::new(KernelError::Internal)
                                .attach_printable("Failed to construct transferred profile")
                        })?;
                        deps.profile_read_model()
                            .update(executor, &transferred_profile)
                            .await?;
                        Ok(transferred_profile)
                    })
                })
                .await?;

            let image_ids: Vec<kernel::prelude::entity::ImageId> = [
                transferred_profile.icon().clone(),
                transferred_profile.banner().clone(),
            ]
            .into_iter()
            .flatten()
            .collect();
            if !image_ids.is_empty() {
                if let Err(error) = self
                    .profile_media_copy_gateway()
                    .request_copy(ProfileMediaCopyRequest {
                        from_account_id: copy_from_account_id,
                        to_account_id: copy_to_account_id,
                        image_ids,
                    })
                    .await
                {
                    tracing::warn!(error = %error, "Profile media copy failed after transfer");
                }
            }

            Ok(())
        }
    }
}

pub trait RejectProfileTransferRequestUseCase: 'static + Sync + Send + Clone {
    fn reject_profile_transfer_request<'a>(
        &'a self,
        auth_id: &'a AuthAccountId,
        request_nanoid: String,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send + 'a;
}

impl<T> RejectProfileTransferRequestUseCase for T
where
    T: 'static
        + Sync
        + Send
        + Clone
        + DependOnAccountQuery
        + DependOnProfileTransferRequestQuery
        + DependOnProfileTransferRequestReadModel
        + DependOnProfileTransferRequestRepository
        + DependOnOrganizationMembershipRepository
        + DependOnDatabaseConnection
        + kernel::interfaces::database::DependOnTransactionManager,
{
    fn reject_profile_transfer_request<'a>(
        &'a self,
        auth_id: &'a AuthAccountId,
        request_nanoid: String,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send + 'a {
        async move {
            let mut connection = self.database_connection().connection().await?;
            let request_nanoid = Nanoid::<ProfileTransferRequest>::new(request_nanoid);
            let projection = self
                .profile_transfer_request_query()
                .find_by_nanoid(&mut connection, &request_nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Profile transfer request not found: {}",
                        request_nanoid.as_ref()
                    ))
                })?;

            let request_id = projection.id().clone();
            let to_org_account_id = projection.to_org_account_id().clone();

            resolve_actor_with_active_membership(
                self,
                auth_id,
                &to_org_account_id,
                &[OrgRole::Owner, OrgRole::Admin],
            )
            .await?;

            let deps = self.clone();
            self.transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        let (request, current_version) = deps
                            .profile_transfer_request_repository()
                            .load(executor, &request_id)
                            .await?
                            .into_parts();
                        if !request.status().is_pending() {
                            return Err(Report::new(KernelError::Rejected)
                                .attach_printable("Profile transfer request is not pending"));
                        }
                        let event = deps
                            .profile_transfer_request_repository()
                            .save(
                                executor,
                                ProfileTransferRequest::reject(request_id, current_version),
                            )
                            .await?;
                        let mut updated_request = Some(request);
                        ProfileTransferRequest::apply(&mut updated_request, event)?;
                        let updated_request = updated_request.ok_or_else(|| {
                            Report::new(KernelError::Internal)
                                .attach_printable("Failed to construct rejected transfer request")
                        })?;
                        deps.profile_transfer_request_read_model()
                            .update(executor, &updated_request)
                            .await?;
                        Ok(())
                    })
                })
                .await
        }
    }
}
