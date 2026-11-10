//! Dedicated internal listener: never mounted into the public router or OpenAPI.
mod config;
#[cfg(test)]
mod kratos_e2e;
mod probe;
#[cfg(test)]
mod tests;
mod worker;

use application::service::registration_enforcement::{
    CheckedRegistration, EnforceRegistrationUseCase, RegistrationAttempt, RegistrationVerdict,
};
use application::service::registration_enforcement_probe::RegistrationEnforcementMonitorUseCase;
use axum::{
    body::to_bytes,
    extract::{ConnectInfo, Request, State},
    http::{Method, StatusCode},
    response::{IntoResponse, Response},
    routing::any,
    Json, Router,
};
pub use config::GuardConfig;
use kernel::interfaces::repository::constant_time_eq;
use serde::Deserialize;
use std::{future::Future, net::SocketAddr, sync::Arc};
use tokio::{net::TcpListener, sync::watch};

/// No Debug: strict allowlist, not a copy of the Kratos request.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookContext {
    flow_id: String,
    flow_type: FlowType,
    email: String,
    email_proof: Option<String>,
    bot_token: Option<String>,
    registration_probe_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum FlowType {
    Browser,
    Api,
}

trait GuardService: Clone + Send + Sync + 'static {
    fn check(&self, context: HookContext) -> impl Future<Output = RegistrationVerdict> + Send;
}

#[derive(Clone)]
struct GuardApi(Arc<crate::handler::AppModule>);
impl GuardService for GuardApi {
    async fn check(&self, context: HookContext) -> RegistrationVerdict {
        if kernel::interfaces::repository::EnforcementProbeId::is_reserved_email(&context.email) {
            let _ = self
                .0
                .receive_registration_enforcement_canary(
                    context.registration_probe_id.as_deref(),
                    &context.flow_id,
                    &context.email,
                )
                .await;
            return RegistrationVerdict::Reject;
        }
        if !self
            .0
            .refresh_registration_enforcement()
            .await
            .is_ok_and(|view| view.state.observation.enforcement_verified)
        {
            return RegistrationVerdict::TemporarilyUnavailable;
        }
        let checked = self
            .0
            .enforce_registration_checked(RegistrationAttempt {
                flow_id: &context.flow_id,
                email: &context.email,
                email_proof: context.email_proof.as_deref(),
                bot_token: context.bot_token.as_deref(),
            })
            .await
            .unwrap_or_else(|_| {
                CheckedRegistration::from(RegistrationVerdict::TemporarilyUnavailable)
            });
        if checked.verdict == RegistrationVerdict::Accept
            && !self
                .0
                .refresh_registration_enforcement()
                .await
                .is_ok_and(|view| checked.covers(&view.decision))
        {
            return RegistrationVerdict::TemporarilyUnavailable;
        }
        checked.verdict
    }
}

#[derive(Clone)]
struct GuardState<S> {
    config: Arc<GuardConfig>,
    service: S,
}

fn denied(temporary: bool) -> Response {
    let text = if temporary {
        "Registration is temporarily unavailable. Please try again later."
    } else {
        "Registration could not be completed. Check the required verification or contact the administrator."
    };
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({
            "messages": [{ "instance_ptr": "#/", "messages": [{
                "id": 4000001, "text": text, "type": "error"
            }] }]
        })),
    )
        .into_response()
}

fn router<S: GuardService>(config: GuardConfig, service: S) -> Router {
    Router::new()
        .route("/registration/pre-persist", any(handle::<S>))
        .fallback(|| async { denied(false) })
        .with_state(GuardState {
            config: Arc::new(config),
            service,
        })
}

async fn handle<S: GuardService>(
    State(state): State<GuardState<S>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
) -> Response {
    // The deadline includes body parsing, database access and provider I/O.
    match tokio::time::timeout(state.config.deadline, check(state, peer, request)).await {
        Ok(response) => response,
        Err(_) => denied(true),
    }
}

async fn check<S: GuardService>(
    state: GuardState<S>,
    peer: SocketAddr,
    request: Request,
) -> Response {
    if request.method() != Method::POST || !state.config.allowed_peers.contains(&peer.ip()) {
        return denied(false);
    }
    let mut credentials = request
        .headers()
        .get_all(axum::http::header::AUTHORIZATION)
        .iter();
    let supplied = credentials
        .next()
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .unwrap_or("");
    if credentials.next().is_some() {
        return denied(false);
    }
    if !constant_time_eq(supplied, &state.config.secret) {
        return denied(false);
    }
    let Ok(body) = to_bytes(request.into_body(), 16 * 1024).await else {
        return denied(false);
    };
    let Ok(context) = serde_json::from_slice::<HookContext>(&body) else {
        return denied(false);
    };
    let _ = context.flow_type; // Parsing restricts this to browser/API.
    if uuid::Uuid::parse_str(&context.flow_id).is_err()
        || kernel::prelude::entity::NormalizedEmail::parse(&context.email).is_err()
        || context.email_proof.as_ref().is_some_and(|v| v.len() > 256)
        || context.bot_token.as_ref().is_some_and(|v| v.len() > 2048)
        || context
            .registration_probe_id
            .as_ref()
            .is_some_and(|v| v.len() > 32)
    {
        return denied(false);
    }
    match state.service.check(context).await {
        RegistrationVerdict::Accept => StatusCode::NO_CONTENT.into_response(),
        RegistrationVerdict::Reject => denied(false),
        RegistrationVerdict::TemporarilyUnavailable => denied(true),
    }
}

pub struct GuardShutdown {
    tx: watch::Sender<bool>,
}
impl GuardShutdown {
    pub fn trigger(&self) {
        let _ = self.tx.send(true);
    }
}

/// Only same-host loopback HTTP is safe without a TLS-terminating private relay.
pub async fn spawn_if_enabled(
    module: Arc<crate::handler::AppModule>,
) -> error_stack::Result<Option<GuardShutdown>, kernel::KernelError> {
    let Some(config) = GuardConfig::from_env().map_err(|message| {
        error_stack::Report::new(kernel::KernelError::Validation).attach_printable(message)
    })?
    else {
        return Ok(None);
    };
    use kernel::interfaces::repository::DependOnRegistrationProtectionPolicy;
    let period = time::Duration::try_from(config.period)
        .map_err(|_| error_stack::Report::new(kernel::KernelError::Validation))?;
    if period >= module.readiness_policy().probe_max_age / 2 {
        return Err(
            error_stack::Report::new(kernel::KernelError::Validation).attach_printable(
                "Registration maintenance period must be less than half probe_max_age",
            ),
        );
    }
    let listener = TcpListener::bind(config.listen).await.map_err(|_| {
        error_stack::Report::new(kernel::KernelError::Internal)
            .attach_printable("Could not bind registration guard listener")
    })?;
    let period = config.period;
    let (tx, mut rx) = watch::channel(false);
    let worker_rx = rx.clone();
    tokio::spawn(worker::run(module.clone(), period, worker_rx));
    let app = router(config, GuardApi(module));
    tokio::spawn(async move {
        let result = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            while !*rx.borrow() {
                if rx.changed().await.is_err() {
                    break;
                }
            }
        })
        .await;
        if result.is_err() {
            tracing::error!("registration guard listener stopped unexpectedly");
        }
    });
    Ok(Some(GuardShutdown { tx }))
}

pub fn spawn_monitor(module: Arc<crate::handler::AppModule>) -> GuardShutdown {
    let (tx, rx) = watch::channel(false);
    tokio::spawn(probe::run(module, rx));
    GuardShutdown { tx }
}
