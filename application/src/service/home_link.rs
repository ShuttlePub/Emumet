//! Current-owner API for the P1 home-link ledger. No relay acceptance/delivery is enabled.
use crate::service::session_context::OrganizationContext;
use error_stack::Report;
use kernel::home_link::{
    DependOnHomeLinkStore, DependOnHomeOriginPolicy, HomeLink, HomeLinkStore, HomeOriginPolicy,
    LinkOwner,
};
use kernel::interfaces::config::DependOnPublicBaseUrl;
use kernel::interfaces::database::{
    DatabaseConnection, DependOnTransactionManager, TransactionManager,
};
use kernel::prelude::entity::AuthAccountId;
use kernel::KernelError;

type Conn<T> = <<T as kernel::interfaces::database::DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Connection;
async fn authorize<T: DependOnHomeLinkStore>(
    deps: &T,
    c: &mut Conn<T>,
    auth: &AuthAccountId,
    org: Option<&OrganizationContext>,
    nanoid: &str,
) -> error_stack::Result<LinkOwner, KernelError> {
    let store = deps.home_link_store();
    let (profile, candidate) = store.owner_candidate(c, nanoid).await?;
    store
        .lock_accounts(c, std::slice::from_ref(&candidate))
        .await?;
    store
        .lock_profiles(c, std::slice::from_ref(&profile))
        .await?;
    let owner = store.owner(c, &profile, &candidate).await?;
    let allowed = if let Some(org) = org {
        org.org_account_id == owner.account_id
    } else {
        store.caller_owns(c, &owner.account_id, auth).await?
    };
    if !allowed || !owner.active {
        return Err(Report::new(KernelError::PermissionDenied));
    }
    Ok(owner)
}

pub trait HomeLinkUseCase:
    DependOnHomeLinkStore
    + DependOnHomeOriginPolicy
    + DependOnPublicBaseUrl
    + DependOnTransactionManager
    + Clone
    + 'static
{
    fn register_home_link(
        &self,
        auth: &AuthAccountId,
        org: Option<&OrganizationContext>,
        nanoid: &str,
        origin: &str,
        mode: i16,
    ) -> impl std::future::Future<Output = error_stack::Result<(HomeLink, bool), KernelError>> + Send
    {
        let deps = self.clone();
        let auth = auth.clone();
        let org = org.cloned();
        let nanoid = nanoid.to_owned();
        let origin = origin.to_owned();
        async move {
            if mode != 3 {
                return Err(Report::new(KernelError::Rejected)
                    .attach_printable("P1 supports trust_mode 3 only"));
            }
            // DNS is outside every DB/gate-bearing transaction.
            let origin = deps.home_origin_policy().validate(&origin).await?;
            let actor = format!(
                "{}/ap/profiles/{}",
                deps.public_base_url().as_str().trim_end_matches('/'),
                nanoid
            );
            for attempt in 0..3 {
                let auth = auth.clone();
                let org = org.clone();
                let nanoid = nanoid.clone();
                let actor = actor.clone();
                let origin = origin.clone();
                let worker = deps.clone();
                let result = deps
                    .transaction_manager()
                    .transaction(move |c| {
                        Box::pin(async move {
                            let owner = authorize(&worker, c, &auth, org.as_ref(), &nanoid).await?;
                            worker
                                .home_link_store()
                                .register(c, &owner, &actor, &origin)
                                .await
                        })
                    })
                    .await;
                if attempt < 2
                    && matches!(&result, Err(e) if matches!(e.current_context(), KernelError::Concurrency))
                {
                    continue; // A fresh transaction re-discovers the owner; never add a reversed Account lock.
                }
                return result;
            }
            unreachable!("bounded owner retries return on the third attempt")
        }
    }
    fn list_home_links(
        &self,
        auth: &AuthAccountId,
        org: Option<&OrganizationContext>,
        nanoid: &str,
    ) -> impl std::future::Future<Output = error_stack::Result<Vec<HomeLink>, KernelError>> + Send
    {
        let deps = self.clone();
        let auth = auth.clone();
        let org = org.cloned();
        let nanoid = nanoid.to_owned();
        async move {
            for attempt in 0..3 {
                let auth = auth.clone();
                let org = org.clone();
                let nanoid = nanoid.clone();
                let worker = deps.clone();
                let result = deps
                    .transaction_manager()
                    .transaction(move |c| {
                        Box::pin(async move {
                            let owner = authorize(&worker, c, &auth, org.as_ref(), &nanoid).await?;
                            worker.home_link_store().list(c, &owner.profile_id).await
                        })
                    })
                    .await;
                if attempt < 2
                    && matches!(&result, Err(e) if matches!(e.current_context(), KernelError::Concurrency))
                {
                    continue; // A fresh transaction re-discovers the owner; never add a reversed Account lock.
                }
                return result;
            }
            unreachable!("bounded owner retries return on the third attempt")
        }
    }
    fn unlink_home_link(
        &self,
        auth: &AuthAccountId,
        org: Option<&OrganizationContext>,
        nanoid: &str,
        id: &str,
    ) -> impl std::future::Future<Output = error_stack::Result<(), KernelError>> + Send {
        let deps = self.clone();
        let auth = auth.clone();
        let org = org.cloned();
        let nanoid = nanoid.to_owned();
        let id = id.to_owned();
        async move {
            for attempt in 0..3 {
                let auth = auth.clone();
                let org = org.clone();
                let nanoid = nanoid.clone();
                let id = id.clone();
                let worker = deps.clone();
                let result = deps
                    .transaction_manager()
                    .transaction(move |c| {
                        Box::pin(async move {
                            let owner = authorize(&worker, c, &auth, org.as_ref(), &nanoid).await?;
                            worker
                                .home_link_store()
                                .unlink(c, &owner.profile_id, &id)
                                .await
                        })
                    })
                    .await;
                if attempt < 2
                    && matches!(&result, Err(e) if matches!(e.current_context(), KernelError::Concurrency))
                {
                    continue; // A fresh transaction re-discovers the owner; never add a reversed Account lock.
                }
                return result;
            }
            unreachable!("bounded owner retries return on the third attempt")
        }
    }
}
impl<T> HomeLinkUseCase for T where
    T: DependOnHomeLinkStore
        + DependOnHomeOriginPolicy
        + DependOnPublicBaseUrl
        + DependOnTransactionManager
        + Clone
        + 'static
{
}
