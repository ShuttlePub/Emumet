mod admin;
mod block_mute;
mod client;
mod follow;
mod follow_relations;
mod organization;
mod unfollow;
pub(crate) use admin::{
    __path_assign_instance_role, __path_ban_account_by_id, __path_revoke_instance_role,
    __path_suspend_account_by_id, __path_unban_account_by_id, __path_unsuspend_account_by_id,
    assign_instance_role, ban_account_by_id, revoke_instance_role, suspend_account_by_id,
    unban_account_by_id, unsuspend_account_by_id,
};
pub(crate) use block_mute::{
    __path_block_account, __path_get_blocks, __path_get_mutes, __path_mute_account,
    __path_unblock_account, __path_unmute_account, block_account, get_blocks, get_mutes,
    mute_account, unblock_account, unmute_account,
};
pub(crate) use client::{
    __path_create_account, __path_deactivate_account_by_id, __path_get_account_by_id,
    __path_get_accounts, __path_reactivate_account_by_id, __path_update_account_by_id,
    create_account, deactivate_account_by_id, get_account_by_id, get_accounts,
    reactivate_account_by_id, update_account_by_id,
};
pub(crate) use follow::{__path_follow_account, follow_account};
pub(crate) use follow_relations::{
    __path_get_followers, __path_get_following, get_followers, get_following,
};
pub(crate) use organization::{
    __path_accept_invite, __path_change_role, __path_create_organization, __path_invite_member,
    __path_list_my_organizations, __path_list_organization_members, __path_remove_member,
    accept_invite, change_role, create_organization, invite_member, list_my_organizations,
    list_organization_members, remove_member,
};
pub(crate) use unfollow::{__path_unfollow_account, unfollow_account};

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
            .route("/accounts/{account_id}", get(get_account_by_id))
            .route("/accounts/{account_id}", patch(update_account_by_id))
            .route("/accounts/{account_id}", delete(deactivate_account_by_id))
            .route(
                "/accounts/{account_id}/reactivate",
                post(reactivate_account_by_id),
            )
            .route("/accounts/{account_id}/follow", post(follow_account))
            .route("/accounts/{account_id}/unfollow", post(unfollow_account))
            .route("/accounts/{account_id}/followers", get(get_followers))
            .route("/accounts/{account_id}/following", get(get_following))
            .route("/accounts/{account_id}/block", post(block_account))
            .route("/accounts/{account_id}/unblock", post(unblock_account))
            .route("/accounts/{account_id}/blocks", get(get_blocks))
            .route("/accounts/{account_id}/mute", post(mute_account))
            .route("/accounts/{account_id}/unmute", post(unmute_account))
            .route("/accounts/{account_id}/mutes", get(get_mutes))
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
