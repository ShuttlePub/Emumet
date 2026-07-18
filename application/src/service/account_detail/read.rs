use crate::permission::{account_view, check_permission};
use crate::transfer::account::{AccountDetailDto, AccountDto, AccountFieldDto};
use crate::transfer::pagination::{apply_pagination, Pagination};
use adapter::processor::account::{AccountQueryProcessor, DependOnAccountQueryProcessor};
use adapter::processor::metadata::{DependOnMetadataQueryProcessor, MetadataQueryProcessor};
use adapter::processor::profile::{DependOnProfileQueryProcessor, ProfileQueryProcessor};
use error_stack::Report;
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::permission::DependOnPermissionChecker;
use kernel::interfaces::repository::{DependOnImageRepository, ImageRepository};
use kernel::prelude::entity::{Account, AccountId, AuthAccountId, Nanoid};
use kernel::KernelError;
use std::collections::{HashMap, HashSet};
use std::future::Future;

pub trait GetAccountDetailUseCase:
    'static
    + Sync
    + Send
    + DependOnAccountQueryProcessor
    + DependOnProfileQueryProcessor
    + DependOnMetadataQueryProcessor
    + DependOnImageRepository
    + DependOnPermissionChecker
{
    fn get_account_detail(
        &self,
        auth_account_id: &AuthAccountId,
        account_nanoid: String,
    ) -> impl Future<Output = error_stack::Result<AccountDetailDto, KernelError>> + Send {
        async move {
            self.get_account_details_by_ids(auth_account_id, vec![account_nanoid.clone()])
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {account_nanoid}"
                    ))
                })
        }
    }

    fn get_all_account_details(
        &self,
        auth_account_id: &AuthAccountId,
        Pagination {
            direction,
            cursor,
            limit,
        }: Pagination<String>,
    ) -> impl Future<Output = error_stack::Result<Vec<AccountDetailDto>, KernelError>> + Send {
        async move {
            let mut executor = self.database_connection().get_executor().await?;
            let accounts = self
                .account_query_processor()
                .find_by_auth_id(&mut executor, auth_account_id)
                .await?;
            let cursor = if let Some(cursor) = cursor {
                self.account_query_processor()
                    .find_by_nanoid(&mut executor, &Nanoid::<Account>::new(cursor))
                    .await?
            } else {
                None
            };
            compose_account_details(
                self,
                &mut executor,
                apply_pagination(accounts, limit, cursor, direction),
            )
            .await
        }
    }

    fn get_account_details_by_ids(
        &self,
        auth_account_id: &AuthAccountId,
        ids: Vec<String>,
    ) -> impl Future<Output = error_stack::Result<Vec<AccountDetailDto>, KernelError>> + Send {
        async move {
            let mut executor = self.database_connection().get_executor().await?;
            let nanoids: Vec<_> = ids.into_iter().map(Nanoid::<Account>::new).collect();
            let accounts = self
                .account_query_processor()
                .find_by_nanoids(&mut executor, &nanoids)
                .await?;
            let mut permitted = Vec::new();
            for account in accounts {
                if check_permission(self, auth_account_id, &account_view(account.id()))
                    .await
                    .is_ok()
                {
                    permitted.push(account);
                }
            }
            compose_account_details(self, &mut executor, permitted).await
        }
    }
}

impl<T> GetAccountDetailUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnAccountQueryProcessor
        + DependOnProfileQueryProcessor
        + DependOnMetadataQueryProcessor
        + DependOnImageRepository
        + DependOnPermissionChecker
{
}

async fn compose_account_details<T>(
    deps: &T,
    executor: &mut <<T as kernel::interfaces::database::DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Executor,
    accounts: Vec<Account>,
) -> error_stack::Result<Vec<AccountDetailDto>, KernelError>
where
    T: DependOnProfileQueryProcessor
        + DependOnMetadataQueryProcessor
        + DependOnImageRepository
        + ?Sized,
{
    if accounts.is_empty() {
        return Ok(Vec::new());
    }
    let account_ids: Vec<_> = accounts
        .iter()
        .map(|account| account.id().clone())
        .collect();
    let profiles = deps
        .profile_query_processor()
        .find_by_account_ids(executor, &account_ids)
        .await?;
    let mut metadata = deps
        .metadata_query_processor()
        .find_by_account_ids(executor, &account_ids)
        .await?;
    metadata.sort_by_key(|field| (*field.account_id().as_ref(), *field.id().as_ref()));
    let image_ids: Vec<_> = profiles
        .iter()
        .flat_map(|profile| {
            profile
                .icon()
                .as_ref()
                .into_iter()
                .chain(profile.banner().as_ref())
                .cloned()
        })
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let images: HashMap<_, _> = deps
        .image_repository()
        .find_by_ids(executor, &image_ids)
        .await?
        .into_iter()
        .map(|image| (image.id().clone(), image.url().as_ref().to_string()))
        .collect();
    let profile_map: HashMap<_, _> = profiles
        .into_iter()
        .map(|profile| (profile.account_id().clone(), profile))
        .collect();
    let mut metadata_map: HashMap<AccountId, Vec<AccountFieldDto>> = HashMap::new();
    for field in metadata {
        metadata_map
            .entry(field.account_id().clone())
            .or_default()
            .push(AccountFieldDto {
                label: field.label().as_ref().to_string(),
                content: field.content().as_ref().to_string(),
            });
    }
    accounts
        .into_iter()
        .map(|account| {
            let account_id = account.id().clone();
            let profile = profile_map.get(&account_id).ok_or_else(|| {
                Report::new(KernelError::NotFound).attach_printable("Profile not found for account")
            })?;
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
                metadata_map.remove(&account_id).unwrap_or_default(),
            ))
        })
        .collect()
}
