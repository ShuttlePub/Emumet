use error_stack::Report;
use kernel::interfaces::permission::{
    AccountRelation, DependOnPermissionChecker, InstanceRole, Permission, PermissionChecker,
    PermissionReq,
};
use kernel::prelude::entity::{AccountId, AuthAccountId};
use kernel::KernelError;

pub fn account_view(account_id: &AccountId) -> Permission {
    Permission::new(PermissionReq::account(
        account_id.clone(),
        [
            AccountRelation::Owner,
            AccountRelation::Editor,
            AccountRelation::Signer,
        ],
    ))
}

pub fn account_edit(account_id: &AccountId) -> Permission {
    Permission::new(PermissionReq::account(
        account_id.clone(),
        [AccountRelation::Owner, AccountRelation::Editor],
    ))
}

pub fn account_deactivate(account_id: &AccountId) -> Permission {
    Permission::new(PermissionReq::account(
        account_id.clone(),
        [AccountRelation::Owner],
    ))
}

pub fn account_sign(account_id: &AccountId) -> Permission {
    Permission::new(PermissionReq::account(
        account_id.clone(),
        [AccountRelation::Owner, AccountRelation::Signer],
    ))
}

pub fn instance_moderate() -> Permission {
    Permission::new(PermissionReq::instance([
        InstanceRole::Admin,
        InstanceRole::Moderator,
    ]))
}

pub async fn check_permission<T: DependOnPermissionChecker + ?Sized>(
    deps: &T,
    subject: &AuthAccountId,
    permission: &Permission,
) -> error_stack::Result<(), KernelError> {
    if !deps
        .permission_checker()
        .satisfies(subject, permission)
        .await?
    {
        return Err(
            Report::new(KernelError::PermissionDenied).attach_printable("Insufficient permissions")
        );
    }
    Ok(())
}
