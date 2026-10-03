use super::*;
use crate::service::profile_transfer::test_support::{active_membership, fixture, Fixture};
use kernel::prelude::entity::{
    AccountId, AccountKind, Nanoid, OrgRole, Profile, ProfileId, ProfileTransferRequestEvent,
    ProfileTransferStatus,
};
use kernel::test_utils::{AccountBuilder, ProfileBuilder};
use kernel::KernelError;
use std::sync::{Arc, Mutex};

fn assert_requested_event(
    f: &Fixture,
) -> (
    ProfileId,
    AccountId,
    AccountId,
    Nanoid<ProfileTransferRequest>,
) {
    let saved = f
        .module
        .transfer_request_repository
        .saved_events
        .lock()
        .unwrap();
    assert_eq!(saved.len(), 1);
    match &saved[0] {
        ProfileTransferRequestEvent::Requested {
            profile_id,
            from_account_id,
            to_org_account_id,
            nanoid,
        } => {
            assert_eq!(profile_id, f.profile.id());
            assert_eq!(from_account_id, f.owner_account.id());
            assert_eq!(to_org_account_id, f.org_account.id());
            (
                profile_id.clone(),
                from_account_id.clone(),
                to_org_account_id.clone(),
                nanoid.clone(),
            )
        }
        _ => panic!("Expected Requested event"),
    }
}

#[tokio::test]
async fn request_creates_pending_transfer() {
    let mut f = fixture();
    f.module.transfer_requests.requests.lock().unwrap().clear();
    f.module.transfer_request_repository.request = None;

    let dto = f
        .module
        .request_profile_transfer(&f.auth, f.profile_nanoid.clone(), "org".to_string())
        .await
        .unwrap();

    assert_eq!(dto.profile_nanoid, f.profile_nanoid);
    assert_eq!(dto.org_account_nanoid, "org");
    assert_eq!(dto.status, "pending");
    assert!(!dto.nanoid.is_empty());

    assert_requested_event(&f);

    let created = f.module.transfer_requests.created.lock().unwrap();
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].profile_id(), f.profile.id());
    assert_eq!(created[0].status(), &ProfileTransferStatus::Pending);
}

#[tokio::test]
async fn request_returns_not_found_for_unknown_profile() {
    let f = fixture();
    let err = f
        .module
        .request_profile_transfer(&f.auth, "unknown".to_string(), "org".to_string())
        .await
        .unwrap_err();
    assert_eq!(err.current_context(), &KernelError::NotFound);
}

#[tokio::test]
async fn request_returns_not_found_for_unknown_org() {
    let f = fixture();
    let err = f
        .module
        .request_profile_transfer(&f.auth, f.profile_nanoid.clone(), "unknown".to_string())
        .await
        .unwrap_err();
    assert_eq!(err.current_context(), &KernelError::NotFound);
}

#[tokio::test]
async fn request_returns_not_found_when_org_nanoid_is_personal_account() {
    let mut f = fixture();
    f.module.accounts.target = Some(f.owner_account.clone());
    let err = f
        .module
        .request_profile_transfer(&f.auth, f.profile_nanoid.clone(), "owner".to_string())
        .await
        .unwrap_err();
    assert_eq!(err.current_context(), &KernelError::NotFound);
}

#[tokio::test]
async fn request_returns_permission_denied_when_actor_does_not_own_profile() {
    let mut f = fixture();
    let other_owner = AccountBuilder::new()
        .id(AccountId::new(999))
        .nanoid(Nanoid::new("other-owner"))
        .kind(AccountKind::Personal)
        .build();
    f.module.accounts.owned_accounts = vec![other_owner.clone()];
    f.module.memberships.lock().unwrap().push(active_membership(
        f.org_account.id().clone(),
        other_owner.id().clone(),
        OrgRole::Member,
    ));
    let err = f
        .module
        .request_profile_transfer(&f.auth, f.profile_nanoid.clone(), "org".to_string())
        .await
        .unwrap_err();
    assert_eq!(err.current_context(), &KernelError::PermissionDenied);
}

#[tokio::test]
async fn request_returns_permission_denied_when_owner_not_active_member() {
    let f = fixture();
    f.module.memberships.lock().unwrap().clear();
    let err = f
        .module
        .request_profile_transfer(&f.auth, f.profile_nanoid.clone(), "org".to_string())
        .await
        .unwrap_err();
    assert_eq!(err.current_context(), &KernelError::PermissionDenied);
}

#[tokio::test]
async fn request_returns_rejected_when_pending_request_already_exists() {
    let f = fixture();
    let err = f
        .module
        .request_profile_transfer(&f.auth, f.profile_nanoid.clone(), "org".to_string())
        .await
        .unwrap_err();
    assert_eq!(err.current_context(), &KernelError::Rejected);
}

#[tokio::test]
async fn request_succeeds_with_second_personal_account_that_owns_profile_and_is_member() {
    let mut f = fixture();
    f.module.transfer_requests.requests.lock().unwrap().clear();
    f.module.transfer_request_repository.request = None;

    let second_owner = AccountBuilder::new()
        .id(AccountId::new(101))
        .nanoid(Nanoid::new("owner"))
        .kind(AccountKind::Personal)
        .build();
    let profile = ProfileBuilder::new()
        .id(ProfileId::new(2))
        .account_id(second_owner.id().clone())
        .nanoid(Nanoid::<Profile>::new("profile-2"))
        .build();

    f.module.accounts.owned_accounts = vec![
        AccountBuilder::new()
            .id(AccountId::new(1000))
            .nanoid(Nanoid::new("another"))
            .kind(AccountKind::Personal)
            .build(),
        second_owner.clone(),
    ];
    f.module.profiles.profiles = Arc::new(Mutex::new(vec![profile.clone().into()]));
    f.module.profile_repository.profile = Some(profile.clone());
    f.module.memberships.lock().unwrap().push(active_membership(
        f.org_account.id().clone(),
        second_owner.id().clone(),
        OrgRole::Member,
    ));

    f.module
        .request_profile_transfer(&f.auth, "profile-2".to_string(), "org".to_string())
        .await
        .unwrap();

    let saved = f
        .module
        .transfer_request_repository
        .saved_events
        .lock()
        .unwrap();
    match &saved[0] {
        ProfileTransferRequestEvent::Requested {
            from_account_id, ..
        } => {
            assert_eq!(from_account_id, second_owner.id());
        }
        _ => panic!("Expected Requested event"),
    }
}
