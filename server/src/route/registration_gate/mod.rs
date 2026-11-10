mod admin;

pub(crate) use admin::{
    __path_close_registration_gate, __path_enable_registration_gate_override,
    __path_get_registration_gate, __path_list_registration_gate_notifications,
    __path_mark_all_registration_gate_notifications_read,
    __path_mark_registration_gate_notifications_read, __path_open_registration_gate,
    __path_revoke_registration_gate_override, __path_unread_registration_gate_notifications,
    close_registration_gate, enable_registration_gate_override, get_registration_gate,
    list_registration_gate_notifications, mark_all_registration_gate_notifications_read,
    mark_registration_gate_notifications_read, open_registration_gate,
    revoke_registration_gate_override, unread_registration_gate_notifications,
};

use crate::handler::AppModule;
use axum::routing::{get, post};
use axum::Router;

pub trait AdminRegistrationGateRouter {
    fn route_admin_registration_gate(self) -> Self;
}

impl AdminRegistrationGateRouter for Router<AppModule> {
    fn route_admin_registration_gate(self) -> Self {
        self.route("/registration-gate", get(get_registration_gate))
            .route("/registration-gate/open", post(open_registration_gate))
            .route("/registration-gate/close", post(close_registration_gate))
            .route(
                "/registration-gate/override",
                post(enable_registration_gate_override),
            )
            .route(
                "/registration-gate/override/revoke",
                post(revoke_registration_gate_override),
            )
            .route(
                "/registration-gate/notifications",
                get(list_registration_gate_notifications),
            )
            .route(
                "/registration-gate/notifications/unread-count",
                get(unread_registration_gate_notifications),
            )
            .route(
                "/registration-gate/notifications/read",
                post(mark_registration_gate_notifications_read),
            )
            .route(
                "/registration-gate/notifications/read-all",
                post(mark_all_registration_gate_notifications_read),
            )
    }
}

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod webhook_tests;
