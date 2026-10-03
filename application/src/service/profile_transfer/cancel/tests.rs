use super::*;
use crate::service::profile_transfer::test_support::fixture;
use kernel::prelude::entity::{
    AccountId, AccountKind, EventVersion, Nanoid, ProfileTransferRequest, ProfileTransferStatus,
};
use kernel::test_utils::AccountBuilder;
use kernel::KernelError;

#[tokio::test]
async fn cancel_succeeds_when_actor_owns_from_account() {
    let f = fixture();
    f.module
        .cancel_profile_transfer_request(&f.auth, f.request_nanoid.clone())
        .await
        .unwrap();

    let saved = f
        .module
        .transfer_request_repository
        .saved_events
        .lock()
        .unwrap();
    assert_eq!(saved.len(), 1);
    assert!(matches!(
        saved[0],
        kernel::prelude::entity::ProfileTransferRequestEvent::Cancelled
    ));
}

#[tokio::test]
async fn cancel_returns_permission_denied_when_actor_does_not_own_from_account() {
    let mut f = fixture();
    let other_account = AccountBuilder::new()
        .id(AccountId::new(500))
        .nanoid(Nanoid::new("other"))
        .kind(AccountKind::Personal)
        .build();
    f.module.accounts.owned_accounts = vec![other_account];

    let err = f
        .module
        .cancel_profile_transfer_request(&f.auth, f.request_nanoid.clone())
        .await
        .unwrap_err();
    assert_eq!(err.current_context(), &KernelError::PermissionDenied);
}

#[tokio::test]
async fn cancel_returns_rejected_when_request_already_decided() {
    let mut f = fixture();
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
        .cancel_profile_transfer_request(&f.auth, f.request_nanoid.clone())
        .await
        .unwrap_err();
    assert_eq!(err.current_context(), &KernelError::Rejected);
}

#[tokio::test]
async fn cancel_returns_not_found_for_unknown_request() {
    let f = fixture();

    let err = f
        .module
        .cancel_profile_transfer_request(&f.auth, "unknown".to_string())
        .await
        .unwrap_err();
    assert_eq!(err.current_context(), &KernelError::NotFound);
}

#[tokio::test]
async fn cancel_updates_read_model() {
    let f = fixture();
    f.module
        .cancel_profile_transfer_request(&f.auth, f.request_nanoid.clone())
        .await
        .unwrap();

    let updated = f.module.transfer_requests.updated.lock().unwrap();
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0].status(), &ProfileTransferStatus::Cancelled);
}
