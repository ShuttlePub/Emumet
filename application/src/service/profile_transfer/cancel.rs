use super::resolve_personal_actor_account_among;
use error_stack::Report;
use kernel::interfaces::database::{
    DatabaseConnection, DependOnDatabaseConnection, TransactionManager,
};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::read_model::{
    DependOnAccountQuery, DependOnProfileTransferRequestQuery,
    DependOnProfileTransferRequestReadModel, ProfileTransferRequestQuery,
    ProfileTransferRequestReadModel,
};
use kernel::interfaces::repository::{
    AggregateRepository, DependOnProfileTransferRequestRepository,
};
use kernel::prelude::entity::{AuthAccountId, Nanoid, ProfileTransferRequest};
use kernel::KernelError;
use std::future::Future;

#[cfg(test)]
mod tests;

pub trait CancelProfileTransferRequestUseCase: 'static + Sync + Send + Clone {
    fn cancel_profile_transfer_request<'a>(
        &'a self,
        auth_id: &'a AuthAccountId,
        request_nanoid: String,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send + 'a;
}

impl<T> CancelProfileTransferRequestUseCase for T
where
    T: 'static
        + Sync
        + Send
        + Clone
        + DependOnAccountQuery
        + DependOnProfileTransferRequestQuery
        + DependOnProfileTransferRequestReadModel
        + DependOnProfileTransferRequestRepository
        + DependOnDatabaseConnection
        + kernel::interfaces::database::DependOnTransactionManager,
{
    fn cancel_profile_transfer_request<'a>(
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
            let from_account_id = projection.from_account_id().clone();

            resolve_personal_actor_account_among(self, auth_id, &from_account_id).await?;

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
                                ProfileTransferRequest::cancel(request_id, current_version),
                            )
                            .await?;
                        let mut updated_request = Some(request);
                        ProfileTransferRequest::apply(&mut updated_request, event)?;
                        let updated_request = updated_request.ok_or_else(|| {
                            Report::new(KernelError::Internal)
                                .attach_printable("Failed to construct cancelled transfer request")
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
