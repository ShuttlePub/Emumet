use crate::dto::organization::{CreateOrganizationDto, OrganizationSummaryDto};
use error_stack::Report;
use kernel::interfaces::database::{
    DatabaseConnection, DependOnTransactionManager, TransactionManager,
};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::read_model::{
    AccountQuery, AccountReadModel, DependOnAccountQuery, DependOnAccountReadModel,
};
use kernel::interfaces::repository::{
    AggregateRepository, DependOnAccountRepository, DependOnOrganizationMembershipRepository,
    OrganizationMembershipRepository,
};
use kernel::prelude::entity::{
    Account, AccountId, AccountIsBot, AccountKind, AccountName, AuthAccountId, CreatedAt, Nanoid,
    OrgRole, OrganizationMembership, OrganizationMembershipStatus,
};
use kernel::KernelError;
use std::future::Future;

pub trait CreateOrganizationUseCase:
    'static
    + Sync
    + Send
    + Clone
    + DependOnAccountQuery
    + DependOnAccountReadModel
    + DependOnAccountRepository
    + DependOnOrganizationMembershipRepository
    + DependOnTransactionManager
{
    fn create_organization(
        &self,
        auth_account_id: AuthAccountId,
        dto: CreateOrganizationDto,
    ) -> impl Future<Output = error_stack::Result<OrganizationSummaryDto, KernelError>> + Send + '_
    {
        async move {
            let mut connection = self.database_connection().connection().await?;
            let creator = self
                .account_query()
                .find_by_auth_id(&mut connection, &auth_account_id)
                .await?
                .into_iter()
                .find(|account| {
                    account.kind() == &AccountKind::Personal && account.deleted_at().is_none()
                })
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound)
                        .attach_printable("Personal account not found for authenticated account")
                })?;

            let account_name = AccountName::new(dto.name);
            let transaction_auth_id = auth_account_id.clone();
            let creator_id = creator.id().clone();
            let deps = self.clone();
            let organization = self
                .transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        let organization_id = AccountId::default();
                        let command = Account::create_organization(
                            organization_id.clone(),
                            account_name,
                            AccountIsBot::new(false),
                            Nanoid::<Account>::default(),
                            transaction_auth_id.clone(),
                        );
                        let event = deps.account_repository().save(executor, command).await?;
                        let mut organization = None;
                        Account::apply(&mut organization, event)?;
                        let organization = organization.ok_or_else(|| {
                            Report::new(KernelError::Internal)
                                .attach_printable("Failed to construct organization account")
                        })?;
                        deps.account_read_model()
                            .create(executor, &organization)
                            .await?;
                        deps.account_read_model()
                            .link_auth_account(executor, &organization_id, &transaction_auth_id)
                            .await?;
                        deps.organization_membership_repository()
                            .create(
                                executor,
                                &OrganizationMembership::new(
                                    organization_id,
                                    creator_id.clone(),
                                    OrgRole::Owner,
                                    OrganizationMembershipStatus::Active,
                                    creator_id,
                                    CreatedAt::now(),
                                ),
                            )
                            .await?;
                        Ok(organization)
                    })
                })
                .await?;

            Ok(OrganizationSummaryDto {
                account_id: organization.nanoid().as_ref().to_string(),
                name: organization.name().as_ref().to_string(),
            })
        }
    }
}

impl<T> CreateOrganizationUseCase for T where
    T: 'static
        + Clone
        + DependOnAccountQuery
        + DependOnAccountReadModel
        + DependOnAccountRepository
        + DependOnOrganizationMembershipRepository
        + DependOnTransactionManager
{
}
