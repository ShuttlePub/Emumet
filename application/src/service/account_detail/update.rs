use super::fields::apply_field_updates;
use super::validate::validate_update_account_dto;
use crate::permission::{account_edit, check_permission};
use crate::service::profile::resolve_field_action_image_id;
use crate::transfer::account::{AccountDetailDto, AccountDto, AccountFieldDto, UpdateAccountDto};
use adapter::processor::account::{AccountQueryProcessor, DependOnAccountQueryProcessor};
use adapter::processor::metadata::{
    DependOnMetadataCommandProcessor, DependOnMetadataQueryProcessor, MetadataQueryProcessor,
};
use adapter::processor::profile::{
    DependOnProfileCommandProcessor, DependOnProfileQueryProcessor, ProfileCommandProcessor,
    ProfileQueryProcessor, UpdateProfileParam,
};
use error_stack::Report;
use kernel::interfaces::database::{
    DatabaseConnection, DependOnTransactionManager, TransactionManager,
};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::event_store::{
    DependOnMetadataEventStore, DependOnProfileEventStore, ProfileEventStore,
};
use kernel::interfaces::permission::DependOnPermissionChecker;
use kernel::interfaces::read_model::{
    AccountReadModel, DependOnAccountReadModel, DependOnMetadataReadModel,
    DependOnProfileReadModel, ProfileReadModel,
};
use kernel::interfaces::repository::{
    AggregateRepository, DependOnAccountRepository, DependOnImageRepository, ImageRepository,
};
use kernel::prelude::entity::{
    Account, AccountIsBot, AuthAccountId, EventId, FieldAction, ImageId, Nanoid, Profile,
    ProfileDisplayName, ProfileSummary,
};
use kernel::KernelError;
use std::collections::HashMap;
use std::future::Future;

pub trait UpdateAccountDetailUseCase:
    'static
    + Sync
    + Send
    + Clone
    + DependOnAccountQueryProcessor
    + DependOnAccountRepository
    + DependOnAccountReadModel
    + DependOnProfileCommandProcessor
    + DependOnProfileQueryProcessor
    + DependOnProfileEventStore
    + DependOnProfileReadModel
    + DependOnMetadataCommandProcessor
    + DependOnMetadataQueryProcessor
    + DependOnMetadataEventStore
    + DependOnMetadataReadModel
    + DependOnImageRepository
    + DependOnPermissionChecker
    + DependOnTransactionManager
{
    fn update_account_detail<'a>(
        &'a self,
        auth_account_id: &'a AuthAccountId,
        dto: UpdateAccountDto,
    ) -> impl Future<Output = error_stack::Result<AccountDetailDto, KernelError>> + Send + 'a {
        async move {
            validate_update_account_dto(&dto)?;
            let mut connection = self.database_connection().connection().await?;
            let projection = self
                .account_query_processor()
                .find_by_nanoid_unfiltered(
                    &mut connection,
                    &Nanoid::<Account>::new(dto.account_nanoid.clone()),
                )
                .await?
                .ok_or_else(|| Report::new(KernelError::NotFound))?;
            check_permission(self, auth_account_id, &account_edit(projection.id())).await?;
            if !projection.status().is_active() {
                return Err(Report::new(KernelError::Rejected)
                    .attach_printable("Cannot modify a suspended or banned account"));
            }

            let account_id = projection.id().clone();
            let deps = self.clone();
            self.transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        let (mut account, version) = deps
                            .account_repository()
                            .load(executor, &account_id)
                            .await?
                            .into_parts();
                        let is_bot = apply_is_bot(*account.is_bot().as_ref(), dto.is_bot);
                        if is_bot != *account.is_bot().as_ref() {
                            let envelope = deps
                                .account_repository()
                                .save(
                                    executor,
                                    Account::update(
                                        account_id.clone(),
                                        AccountIsBot::new(is_bot),
                                        version,
                                    ),
                                )
                                .await?;
                            let mut updated_account = Some(account);
                            Account::apply(&mut updated_account, envelope)?;
                            account = updated_account
                                .ok_or_else(|| Report::new(KernelError::Internal))?;
                            deps.account_read_model().update(executor, &account).await?;
                        }

                        let profile = deps
                            .profile_query_processor()
                            .find_by_account_id(executor, &account_id)
                            .await?
                            .ok_or_else(|| Report::new(KernelError::NotFound))?;
                        let icon =
                            resolve_field_action_image_id(&deps, executor, &dto.icon_url).await?;
                        let banner =
                            resolve_field_action_image_id(&deps, executor, &dto.banner_url).await?;
                        if !dto.display_name.is_unchanged()
                            || !dto.summary.is_unchanged()
                            || !icon.is_unchanged()
                            || !banner.is_unchanged()
                        {
                            deps.profile_command_processor()
                                .update(
                                    executor,
                                    UpdateProfileParam {
                                        profile_id: profile.id().clone(),
                                        display_name: dto
                                            .display_name
                                            .clone()
                                            .map(ProfileDisplayName::new),
                                        summary: dto.summary.clone().map(ProfileSummary::new),
                                        icon,
                                        banner,
                                    },
                                )
                                .await?;
                            let profile = rehydrate_profile(&deps, executor, profile.id()).await?;
                            deps.profile_read_model().update(executor, &profile).await?;
                        }

                        let mut existing_fields = deps
                            .metadata_query_processor()
                            .find_by_account_id(executor, &account_id)
                            .await?;
                        existing_fields.sort_by_key(|field| *field.id().as_ref());
                        if let Some(fields) = &dto.fields {
                            apply_field_updates(
                                &deps,
                                executor,
                                &account_id,
                                &existing_fields,
                                fields,
                            )
                            .await?;
                        }

                        let account = deps
                            .account_query_processor()
                            .find_by_id(executor, &account_id)
                            .await?
                            .ok_or_else(|| Report::new(KernelError::NotFound))?;
                        let profile = deps
                            .profile_query_processor()
                            .find_by_account_id(executor, &account_id)
                            .await?
                            .ok_or_else(|| Report::new(KernelError::NotFound))?;
                        let mut current_fields = deps
                            .metadata_query_processor()
                            .find_by_account_id(executor, &account_id)
                            .await?;
                        current_fields.sort_by_key(|field| *field.id().as_ref());
                        let fields = current_fields
                            .into_iter()
                            .map(|field| AccountFieldDto {
                                label: field.label().as_ref().to_string(),
                                content: field.content().as_ref().to_string(),
                            })
                            .collect();
                        let image_ids: Vec<_> = profile
                            .icon()
                            .as_ref()
                            .into_iter()
                            .chain(profile.banner().as_ref())
                            .cloned()
                            .collect();
                        let images: HashMap<ImageId, String> = deps
                            .image_repository()
                            .find_by_ids(executor, &image_ids)
                            .await?
                            .into_iter()
                            .map(|image| (image.id().clone(), image.url().as_ref().to_string()))
                            .collect();
                        Ok(AccountDto::from(account).into_detail(
                            profile
                                .display_name()
                                .as_ref()
                                .map(|v| v.as_ref().to_string()),
                            profile.summary().as_ref().map(|v| v.as_ref().to_string()),
                            profile
                                .icon()
                                .as_ref()
                                .and_then(|id| images.get(id).cloned()),
                            profile
                                .banner()
                                .as_ref()
                                .and_then(|id| images.get(id).cloned()),
                            fields,
                        ))
                    })
                })
                .await
        }
    }
}

impl<T> UpdateAccountDetailUseCase for T where
    T: 'static
        + Clone
        + Sync
        + Send
        + DependOnAccountQueryProcessor
        + DependOnAccountRepository
        + DependOnAccountReadModel
        + DependOnProfileCommandProcessor
        + DependOnProfileQueryProcessor
        + DependOnProfileEventStore
        + DependOnProfileReadModel
        + DependOnMetadataCommandProcessor
        + DependOnMetadataQueryProcessor
        + DependOnMetadataEventStore
        + DependOnMetadataReadModel
        + DependOnImageRepository
        + DependOnPermissionChecker
        + DependOnTransactionManager
        + Sync
        + Send
{
}

fn apply_is_bot(current: bool, action: FieldAction<bool>) -> bool {
    match action {
        FieldAction::Unchanged => current,
        FieldAction::Clear => false,
        FieldAction::Set(value) => value,
    }
}

async fn rehydrate_profile<T>(
    deps: &T,
    executor: &mut <<T as kernel::interfaces::database::DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Connection,
    profile_id: &kernel::prelude::entity::ProfileId,
) -> error_stack::Result<Profile, KernelError>
where
    T: DependOnProfileEventStore + ?Sized,
{
    let events = deps
        .profile_event_store()
        .find_by_id(executor, &EventId::from(profile_id.clone()), None)
        .await?;
    let mut profile = None;
    for event in events {
        Profile::apply(&mut profile, event)?;
    }
    profile.ok_or_else(|| Report::new(KernelError::NotFound))
}
