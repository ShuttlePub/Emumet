use error_stack::Report;
use kernel::interfaces::permission::{
    DependOnPermissionChecker, Permission, PermissionChecker, PermissionReq, Relation, Resource,
};
use kernel::prelude::entity::{AccountId, AuthAccountId};
use kernel::KernelError;

pub fn account_view(account_id: &AccountId) -> Permission {
    Permission::new(PermissionReq::new(
        Resource::Account(account_id.clone()),
        [Relation::Owner, Relation::Editor, Relation::Signer],
    ))
}

pub fn account_edit(account_id: &AccountId) -> Permission {
    Permission::new(PermissionReq::new(
        Resource::Account(account_id.clone()),
        [Relation::Owner, Relation::Editor],
    ))
}

pub fn account_deactivate(account_id: &AccountId) -> Permission {
    Permission::new(PermissionReq::new(
        Resource::Account(account_id.clone()),
        [Relation::Owner],
    ))
}

pub fn account_sign(account_id: &AccountId) -> Permission {
    Permission::new(PermissionReq::new(
        Resource::Account(account_id.clone()),
        [Relation::Owner, Relation::Signer],
    ))
}

pub fn instance_moderate() -> Permission {
    Permission::new(PermissionReq::new(
        Resource::Instance,
        [Relation::Admin, Relation::Moderator],
    ))
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
