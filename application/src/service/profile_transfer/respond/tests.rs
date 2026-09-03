use super::*;
use crate::service::profile_transfer::test_support::{
    active_membership, fixture, profile_with_media, Fixture,
};
use kernel::prelude::entity::{
    AccountId, AccountKind, EventVersion, ImageId, Nanoid, OrgRole, ProfileEvent,
    ProfileTransferRequest, ProfileTransferRequestEvent, ProfileTransferStatus,
};
use kernel::test_utils::AccountBuilder;
use kernel::KernelError;
use std::sync::{Arc, Mutex};

fn assert_accept_event(f: &Fixture) {
    let saved = f
        .module
        .transfer_request_repository
        .saved_events
        .lock()
        .unwrap();
    assert_eq!(saved.len(), 1);
    assert!(matches!(saved[0], ProfileTransferRequestEvent::Accepted));
}

fn assert_transfer_event(f: &Fixture) -> (AccountId, AccountId) {
    let saved = f.module.profile_repository.saved_events.lock().unwrap();
    assert_eq!(saved.len(), 1);
    match &saved[0] {
        ProfileEvent::AccountTransferred {
            from_account_id,
            to_account_id,
        } => (from_account_id.clone(), to_account_id.clone()),
        _ => panic!("Expected AccountTransferred event"),
    }
}

fn add_org_actor(f: &mut Fixture, account_id: AccountId, role: OrgRole) {
    let account = AccountBuilder::new()
        .id(account_id.clone())
        .nanoid(Nanoid::new("actor"))
        .kind(AccountKind::Personal)
        .build();
    f.module.accounts.owned_accounts.push(account);
    f.module.memberships.lock().unwrap().push(active_membership(
        f.org_account.id().clone(),
        account_id,
        role,
    ));
}

#[tokio::test]
async fn accept_succeeds_as_owner_with_actor_different_from_org_account() {
    let mut f = fixture();
    let actor_id = AccountId::new(777);
    add_org_actor(&mut f, actor_id.clone(), OrgRole::Owner);

    f.module
        .accept_profile_transfer_request(&f.auth, f.request_nanoid.clone())
        .await
        .unwrap();

    assert_accept_event(&f);
    let (from, to) = assert_transfer_event(&f);
    assert_eq!(from, *f.owner_account.id());
    assert_eq!(to, *f.org_account.id());
    assert_ne!(
        actor_id,
        *f.org_account.id(),
        "regression lock: actor != org id"
    );

    let updated = f.module.profiles.updated.lock().unwrap();
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0].account_id(), f.org_account.id());
}

#[tokio::test]
async fn accept_succeeds_as_admin_with_actor_different_from_org_account() {
    let mut f = fixture();
    let actor_id = AccountId::new(778);
    add_org_actor(&mut f, actor_id.clone(), OrgRole::Admin);

    f.module
        .accept_profile_transfer_request(&f.auth, f.request_nanoid.clone())
        .await
        .unwrap();

    assert_accept_event(&f);
    assert_transfer_event(&f);
    assert_ne!(
        actor_id,
        *f.org_account.id(),
        "regression lock: actor != org id"
    );
}

#[tokio::test]
async fn accept_copies_media_when_icon_and_banner_exist() {
    let mut f = fixture();
    let actor_id = AccountId::new(779);
    add_org_actor(&mut f, actor_id.clone(), OrgRole::Owner);

    let icon_id = ImageId::new(10);
    let banner_id = ImageId::new(11);
    let profile = profile_with_media(
        f.profile.id().clone(),
        f.owner_account.id().clone(),
        f.profile.nanoid().clone(),
        icon_id.clone(),
        banner_id.clone(),
    );
    f.module.profile_repository.profile = Some(profile.clone());
    f.module.profiles.profiles = Arc::new(Mutex::new(vec![profile.into()]));

    f.module
        .accept_profile_transfer_request(&f.auth, f.request_nanoid.clone())
        .await
        .unwrap();

    let recorded = f.module.media_copy_gateway.recorded.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].from_account_id, *f.owner_account.id());
    assert_eq!(recorded[0].to_account_id, *f.org_account.id());
    assert!(recorded[0].image_ids.contains(&icon_id));
    assert!(recorded[0].image_ids.contains(&banner_id));
}

#[tokio::test]
async fn accept_skips_media_copy_when_no_images() {
    let mut f = fixture();
    let actor_id = AccountId::new(780);
    add_org_actor(&mut f, actor_id.clone(), OrgRole::Owner);

    f.module
        .accept_profile_transfer_request(&f.auth, f.request_nanoid.clone())
        .await
        .unwrap();

    let recorded = f.module.media_copy_gateway.recorded.lock().unwrap();
    assert!(recorded.is_empty());
}

#[tokio::test]
async fn accept_returns_rejected_when_request_already_decided() {
    let mut f = fixture();
    let actor_id = AccountId::new(781);
    add_org_actor(&mut f, actor_id.clone(), OrgRole::Owner);

    let decided = {
        let request = f
            .module
            .transfer_request_repository
            .request
            .as_ref()
            .unwrap();
        ProfileTransferRequest::new(
            request.id().clone(),
            f.profile.id().clone(),
            f.owner_account.id().clone(),
            f.org_account.id().clone(),
            ProfileTransferStatus::Accepted,
            EventVersion::default(),
            request.nanoid().clone(),
        )
    };
    f.module.transfer_request_repository.request = Some(decided);

    let err = f
        .module
        .accept_profile_transfer_request(&f.auth, f.request_nanoid.clone())
        .await
        .unwrap_err();
    assert_eq!(err.current_context(), &KernelError::Rejected);
}

#[tokio::test]
async fn accept_returns_permission_denied_for_member_role() {
    let mut f = fixture();
    let actor_id = AccountId::new(782);
    f.module.accounts.owned_accounts.clear();
    add_org_actor(&mut f, actor_id.clone(), OrgRole::Member);

    let err = f
        .module
        .accept_profile_transfer_request(&f.auth, f.request_nanoid.clone())
        .await
        .unwrap_err();
    assert_eq!(err.current_context(), &KernelError::PermissionDenied);
}

#[tokio::test]
async fn accept_returns_permission_denied_for_non_member() {
    let mut f = fixture();
    f.module.accounts.owned_accounts.clear();
    f.module.accounts.owned_accounts.push(
        AccountBuilder::new()
            .id(AccountId::new(783))
            .nanoid(Nanoid::new("stranger"))
            .kind(AccountKind::Personal)
            .build(),
    );

    let err = f
        .module
        .accept_profile_transfer_request(&f.auth, f.request_nanoid.clone())
        .await
        .unwrap_err();
    assert_eq!(err.current_context(), &KernelError::PermissionDenied);
}

#[tokio::test]
async fn accept_returns_not_found_for_unknown_request() {
    let mut f = fixture();
    let actor_id = AccountId::new(784);
    add_org_actor(&mut f, actor_id.clone(), OrgRole::Owner);

    let err = f
        .module
        .accept_profile_transfer_request(&f.auth, "unknown".to_string())
        .await
        .unwrap_err();
    assert_eq!(err.current_context(), &KernelError::NotFound);
}

fn assert_reject_event(f: &Fixture) {
    let saved = f
        .module
        .transfer_request_repository
        .saved_events
        .lock()
        .unwrap();
    assert_eq!(saved.len(), 1);
    assert!(matches!(saved[0], ProfileTransferRequestEvent::Rejected));
}

#[tokio::test]
async fn reject_succeeds_as_owner_with_actor_different_from_org_account() {
    let mut f = fixture();
    let actor_id = AccountId::new(785);
    add_org_actor(&mut f, actor_id.clone(), OrgRole::Owner);

    f.module
        .reject_profile_transfer_request(&f.auth, f.request_nanoid.clone())
        .await
        .unwrap();

    assert_reject_event(&f);
    assert!(f
        .module
        .profile_repository
        .saved_events
        .lock()
        .unwrap()
        .is_empty());
    assert_ne!(
        actor_id,
        *f.org_account.id(),
        "regression lock: actor != org id"
    );
}

#[tokio::test]
async fn reject_succeeds_as_admin_with_actor_different_from_org_account() {
    let mut f = fixture();
    let actor_id = AccountId::new(786);
    add_org_actor(&mut f, actor_id.clone(), OrgRole::Admin);

    f.module
        .reject_profile_transfer_request(&f.auth, f.request_nanoid.clone())
        .await
        .unwrap();

    assert_reject_event(&f);
}

#[tokio::test]
async fn reject_returns_permission_denied_for_member_role() {
    let mut f = fixture();
    let actor_id = AccountId::new(787);
    f.module.accounts.owned_accounts.clear();
    add_org_actor(&mut f, actor_id.clone(), OrgRole::Member);

    let err = f
        .module
        .reject_profile_transfer_request(&f.auth, f.request_nanoid.clone())
        .await
        .unwrap_err();
    assert_eq!(err.current_context(), &KernelError::PermissionDenied);
}

#[tokio::test]
async fn reject_returns_permission_denied_for_non_member() {
    let mut f = fixture();
    f.module.accounts.owned_accounts.clear();
    f.module.accounts.owned_accounts.push(
        AccountBuilder::new()
            .id(AccountId::new(788))
            .nanoid(Nanoid::new("stranger"))
            .kind(AccountKind::Personal)
            .build(),
    );

    let err = f
        .module
        .reject_profile_transfer_request(&f.auth, f.request_nanoid.clone())
        .await
        .unwrap_err();
    assert_eq!(err.current_context(), &KernelError::PermissionDenied);
}

#[tokio::test]
async fn reject_returns_rejected_when_request_already_decided() {
    let mut f = fixture();
    let actor_id = AccountId::new(789);
    add_org_actor(&mut f, actor_id.clone(), OrgRole::Owner);

    let decided = {
        let request = f
            .module
            .transfer_request_repository
            .request
            .as_ref()
            .unwrap();
        ProfileTransferRequest::new(
            request.id().clone(),
            f.profile.id().clone(),
            f.owner_account.id().clone(),
            f.org_account.id().clone(),
            ProfileTransferStatus::Accepted,
            EventVersion::default(),
            request.nanoid().clone(),
        )
    };
    f.module.transfer_request_repository.request = Some(decided);

    let err = f
        .module
        .reject_profile_transfer_request(&f.auth, f.request_nanoid.clone())
        .await
        .unwrap_err();
    assert_eq!(err.current_context(), &KernelError::Rejected);
}

#[tokio::test]
async fn reject_returns_not_found_for_unknown_request() {
    let mut f = fixture();
    let actor_id = AccountId::new(790);
    add_org_actor(&mut f, actor_id.clone(), OrgRole::Owner);

    let err = f
        .module
        .reject_profile_transfer_request(&f.auth, "unknown".to_string())
        .await
        .unwrap_err();
    assert_eq!(err.current_context(), &KernelError::NotFound);
}

#[tokio::test]
async fn reject_updates_read_model() {
    let mut f = fixture();
    let actor_id = AccountId::new(791);
    add_org_actor(&mut f, actor_id.clone(), OrgRole::Owner);

    f.module
        .reject_profile_transfer_request(&f.auth, f.request_nanoid.clone())
        .await
        .unwrap();

    let updated = f.module.transfer_requests.updated.lock().unwrap();
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0].status(), &ProfileTransferStatus::Rejected);
}
