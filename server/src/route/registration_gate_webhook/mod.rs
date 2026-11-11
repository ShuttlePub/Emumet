mod admin;

pub(crate) use admin::{
    __path_create_registration_gate_webhook_endpoint,
    __path_delete_registration_gate_webhook_endpoint,
    __path_get_registration_gate_webhook_endpoint, __path_list_registration_gate_webhook_endpoints,
    __path_list_registration_gate_webhook_failures, __path_retry_registration_gate_webhooks,
    __path_send_registration_gate_webhook_test, __path_update_registration_gate_webhook_endpoint,
    create_registration_gate_webhook_endpoint, delete_registration_gate_webhook_endpoint,
    get_registration_gate_webhook_endpoint, list_registration_gate_webhook_endpoints,
    list_registration_gate_webhook_failures, retry_registration_gate_webhooks,
    send_registration_gate_webhook_test, update_registration_gate_webhook_endpoint,
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
            "/registration-gate/webhook/endpoints",
            get(list_registration_gate_webhook_endpoints)
                .post(create_registration_gate_webhook_endpoint),
        )
        .route(
            "/registration-gate/webhook/endpoints/{endpoint_id}",
            get(get_registration_gate_webhook_endpoint)
                .put(update_registration_gate_webhook_endpoint),
        )
        .route(
            "/registration-gate/webhook/endpoints/{endpoint_id}/delete",
            post(delete_registration_gate_webhook_endpoint),
        )
        .route(
            "/registration-gate/webhook/endpoints/{endpoint_id}/test",
            post(send_registration_gate_webhook_test),
        )
        .route(
            "/registration-gate/webhook/retry",
            post(retry_registration_gate_webhooks),
        )
        .route(
            "/registration-gate/webhook/failures",
            get(list_registration_gate_webhook_failures),
        )
    }
}
