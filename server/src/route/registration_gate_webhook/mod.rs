mod admin;

pub(crate) use admin::{
    __path_get_registration_gate_webhook_status, __path_retry_registration_gate_webhooks,
    __path_send_registration_gate_webhook_test, get_registration_gate_webhook_status,
    retry_registration_gate_webhooks, send_registration_gate_webhook_test,
};

use crate::handler::AppModule;
use axum::routing::{get, post};
use axum::Router;

pub trait AdminRegistrationGateWebhookRouter {
    fn route_admin_registration_gate_webhook(self) -> Self;
}

impl AdminRegistrationGateWebhookRouter for Router<AppModule> {
    fn route_admin_registration_gate_webhook(self) -> Self {
        self.route(
            "/registration-gate/webhook",
            get(get_registration_gate_webhook_status),
        )
        .route(
            "/registration-gate/webhook/test",
            post(send_registration_gate_webhook_test),
        )
        .route(
            "/registration-gate/webhook/retry",
            post(retry_registration_gate_webhooks),
        )
    }
}
