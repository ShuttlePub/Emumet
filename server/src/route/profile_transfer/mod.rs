mod client;

pub(crate) use client::{
    __path_accept_profile_transfer_request, __path_cancel_profile_transfer_request,
    __path_create_profile_transfer_request, __path_reject_profile_transfer_request,
    accept_profile_transfer_request, cancel_profile_transfer_request,
    create_profile_transfer_request, reject_profile_transfer_request,
};

use crate::handler::AppModule;
use axum::routing::post;
use axum::Router;

pub trait ProfileTransferRouter {
    fn route_profile_transfers(self) -> Self;
}

impl ProfileTransferRouter for Router<AppModule> {
    fn route_profile_transfers(self) -> Self {
        self.route(
            "/profiles/{profile_nanoid}/transfer-requests",
            post(create_profile_transfer_request),
        )
        .route(
            "/profile-transfer-requests/{request_nanoid}/accept",
            post(accept_profile_transfer_request),
        )
        .route(
            "/profile-transfer-requests/{request_nanoid}/reject",
            post(reject_profile_transfer_request),
        )
        .route(
            "/profile-transfer-requests/{request_nanoid}/cancel",
            post(cancel_profile_transfer_request),
        )
    }
}
