mod admin;
mod client;
mod organization;
pub(crate) use admin::{
    __path_assign_instance_role, __path_ban_account_by_id, __path_list_account_warnings,
    __path_revoke_instance_role, __path_suspend_account_by_id, __path_unban_account_by_id,
    __path_unsuspend_account_by_id, __path_warn_account_by_id, assign_instance_role,
    ban_account_by_id, list_account_warnings, revoke_instance_role, suspend_account_by_id,
    unban_account_by_id, unsuspend_account_by_id, warn_account_by_id,
};
pub(crate) use client::{
    __path_create_account, __path_create_profile, __path_deactivate_account_by_id,
    __path_get_account_by_id, __path_get_accounts, __path_reactivate_account_by_id,
    __path_set_profile_username, __path_update_account_by_id, create_account, create_profile,
    deactivate_account_by_id, get_account_by_id, get_accounts, reactivate_account_by_id,
    set_profile_username, update_account_by_id,
};
pub(crate) use organization::{
    __path_accept_invite, __path_change_role, __path_create_organization, __path_invite_member,
    __path_list_my_organizations, __path_list_organization_members, __path_remove_member,
    accept_invite, change_role, create_organization, invite_member, list_my_organizations,
    list_organization_members, remove_member,
};

use crate::handler::AppModule;
use axum::routing::{delete, get, patch, post, put};
use axum::Router;

pub trait AccountRouter {
    fn route_account(self) -> Self;
}

pub trait AdminAccountRouter {
    fn route_admin_account(self) -> Self;
}

pub trait OrgAccountRouter {
    fn route_org_account(self) -> Self;
}

impl AccountRouter for Router<AppModule> {
    fn route_account(self) -> Self {
        self.route("/accounts", get(get_accounts))
            .route("/accounts", post(create_account))
            .route("/profiles", post(create_profile))
            .route(
                "/profiles/{profile_nanoid}/username",
                post(set_profile_username),
            )
            .route("/accounts/{account_id}", get(get_account_by_id))
            .route("/accounts/{account_id}", patch(update_account_by_id))
            .route("/accounts/{account_id}", delete(deactivate_account_by_id))
            .route(
                "/accounts/{account_id}/reactivate",
                post(reactivate_account_by_id),
            )
    }
}

impl OrgAccountRouter for Router<AppModule> {
    fn route_org_account(self) -> Self {
        self.route("/organizations", post(create_organization))
            .route("/me/organizations", get(list_my_organizations))
            .route(
                "/organizations/{org}/members",
                get(list_organization_members),
            )
            .route("/organizations/{org}/invites", post(invite_member))
            .route(
                "/organizations/{org}/invites/{account_id}/accept",
                post(accept_invite),
            )
            .route(
                "/organizations/{org}/members/{account_id}/role",
                put(change_role),
            )
            .route(
                "/organizations/{org}/members/{account_id}",
                delete(remove_member),
            )
    }
}

impl AdminAccountRouter for Router<AppModule> {
    fn route_admin_account(self) -> Self {
        self.route(
            "/accounts/{account_id}/warnings",
            get(list_account_warnings).post(warn_account_by_id),
        )
        .route(
            "/accounts/{account_id}/suspend",
            post(suspend_account_by_id),
        )
        .route(
            "/accounts/{account_id}/unsuspend",
            post(unsuspend_account_by_id),
        )
        .route("/accounts/{account_id}/ban", post(ban_account_by_id))
        .route("/accounts/{account_id}/unban", post(unban_account_by_id))
        .route(
            "/accounts/{account_id}/roles/{role}",
            put(assign_instance_role).delete(revoke_instance_role),
        )
    }
}
