//! followers / following reads of a Profile (P3i, E8 contract).
//!
//! Owner-only like every Profile-scoped read: the owner check, the Profile row lock and the reads
//! run in one transaction (see `domain_control::require_profile_owner`), so a Profile transfer
//! cannot interleave. Only the listed Profile's own active generations are returned, so other
//! Profiles' and other Accounts' relations never appear.

use crate::dto::profile_inbound::{
    ProfileActorRefDto, ProfileFollowDirection, ProfileFollowItemDto, ProfileFollowPageDto,
    ProfileFollowStateDto,
};
use crate::service::domain_control::require_profile_owner;
use crate::service::session_context::OrganizationContext;
use error_stack::Report;
use kernel::activitypub::ActorUrlBuilder;
use kernel::interfaces::config::DependOnPublicBaseUrl;
use kernel::interfaces::database::{DependOnTransactionManager, TransactionManager};
use kernel::interfaces::read_model::{
    DependOnAccountQuery, DependOnProfileReadModel, ProfileReadModel,
};
use kernel::interfaces::repository::{
    DependOnDomainControlRepository, DependOnProfileFollowRepository, ProfileFollowRepository,
};
use kernel::prelude::entity::{AuthAccountId, ProfileFollowId, ProfileGraphActor};
use kernel::KernelError;
use std::future::Future;

pub const DEFAULT_PAGE_SIZE: u32 = 20;
pub const MAX_PAGE_SIZE: u32 = 100;

fn parse_cursor(cursor: Option<&str>) -> error_stack::Result<Option<ProfileFollowId>, KernelError> {
    cursor
        .map(|raw| {
            raw.parse::<i64>()
                .ok()
                .filter(|id| *id > 0)
                .map(ProfileFollowId::new)
                .ok_or_else(|| {
                    Report::new(KernelError::Validation)
                        .attach_printable("cursor is not a valid follow list cursor".to_string())
                })
        })
        .transpose()
}

pub trait ProfileFollowListUseCase:
    'static
    + Sync
    + Send
    + Clone
    + DependOnAccountQuery
    + DependOnDomainControlRepository
    + DependOnProfileFollowRepository
    + DependOnProfileReadModel
    + DependOnTransactionManager
    + DependOnPublicBaseUrl
{
    /// Active (pending / accepted) follow generations of the Profile, oldest first. `limit` is
    /// clamped to `1..=MAX_PAGE_SIZE`; `cursor` is the `next_cursor` of the previous page.
    fn list_profile_follows<'a>(
        &'a self,
        auth_account_id: &'a AuthAccountId,
        org_context: Option<&'a OrganizationContext>,
        profile_nanoid: &'a str,
        direction: ProfileFollowDirection,
        cursor: Option<&'a str>,
        limit: Option<u32>,
    ) -> impl Future<Output = error_stack::Result<ProfileFollowPageDto, KernelError>> + Send + 'a
    {
        async move {
            let after = parse_cursor(cursor)?;
            let limit = limit.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE);
            let deps = self.clone();
            let auth_account_id = auth_account_id.clone();
            let org_context = org_context.cloned();
            let profile_nanoid = profile_nanoid.to_string();
            self.transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        let profile_id = require_profile_owner(
                            &deps,
                            executor,
                            &auth_account_id,
                            org_context.as_ref(),
                            &profile_nanoid,
                        )
                        .await?;
                        let follows = deps.profile_follow_repository();
                        // One extra row tells whether another page follows.
                        let fetch = i64::from(limit) + 1;
                        let mut rows = match direction {
                            ProfileFollowDirection::Following => {
                                follows
                                    .list_following(executor, &profile_id, after.as_ref(), fetch)
                                    .await?
                            }
                            ProfileFollowDirection::Followers => {
                                follows
                                    .list_followers(executor, &profile_id, after.as_ref(), fetch)
                                    .await?
                            }
                        };
                        let has_more = rows.len() > limit as usize;
                        rows.truncate(limit as usize);
                        let next_cursor = has_more
                            .then(|| rows.last().map(|row| row.id().as_ref().to_string()))
                            .flatten();

                        let base = deps.public_base_url().as_str().to_string();
                        let mut items = Vec::with_capacity(rows.len());
                        for row in &rows {
                            let Some(state) = ProfileFollowStateDto::from_active(row.state())
                            else {
                                continue;
                            };
                            let counterpart = match direction.counterpart(row) {
                                ProfileGraphActor::Remote(url) => ProfileActorRefDto::Remote {
                                    actor_url: url.clone(),
                                },
                                ProfileGraphActor::Local(local_id) => {
                                    let Some(local) = deps
                                        .profile_read_model()
                                        .find_by_id_unfiltered(executor, local_id)
                                        .await?
                                    else {
                                        continue;
                                    };
                                    let nanoid = local.nanoid().as_ref().to_string();
                                    ProfileActorRefDto::Local {
                                        actor_url: ActorUrlBuilder::for_profile(&base, &nanoid)
                                            .actor_id(),
                                        profile_nanoid: nanoid,
                                    }
                                }
                            };
                            items.push(ProfileFollowItemDto {
                                id: row.nanoid().as_ref().to_string(),
                                profile_nanoid: profile_nanoid.clone(),
                                counterpart,
                                state,
                                created_at: *row.created_at(),
                            });
                        }
                        Ok(ProfileFollowPageDto { items, next_cursor })
                    })
                })
                .await
        }
    }
}

impl<T> ProfileFollowListUseCase for T where
    T: 'static
        + Sync
        + Send
        + Clone
        + DependOnAccountQuery
        + DependOnDomainControlRepository
        + DependOnProfileFollowRepository
        + DependOnProfileReadModel
        + DependOnTransactionManager
        + DependOnPublicBaseUrl
{
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_accepts_positive_numbers_only() {
        assert_eq!(parse_cursor(None).unwrap(), None);
        assert_eq!(
            parse_cursor(Some("42")).unwrap(),
            Some(ProfileFollowId::new(42))
        );
        for bad in ["", "0", "-1", "abc", "1.5", "9223372036854775808"] {
            assert!(parse_cursor(Some(bad)).is_err(), "{bad}");
        }
    }
}
