use super::registration_gate::{RecordRegistrationEnforcementUseCase, RegistrationGateView};
use error_stack::Report;
use kernel::interfaces::database::{DatabaseConnection, TransactionManager};
use kernel::interfaces::repository::{
    apply_gate_observation, DependOnRegistrationEnforcementProbeClient,
    DependOnRegistrationEnforcementProbeRepository, EnforcementCanary, EnforcementProbeEvidence,
    EnforcementProbeId, EnforcementProbeMode, RegistrationEnforcementProbeClient,
    RegistrationEnforcementProbeRepository, RegistrationGateRepository,
};
use kernel::KernelError;
use rand::RngCore;
use std::future::Future;
use time::OffsetDateTime;

fn probe_id() -> EnforcementProbeId {
    let mut bytes = [0_u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    EnforcementProbeId::parse(&bytes.iter().map(|b| format!("{b:02x}")).collect::<String>())
        .expect("generated lowercase hex")
}

pub trait RegistrationEnforcementMonitorUseCase:
    RecordRegistrationEnforcementUseCase
    + DependOnRegistrationEnforcementProbeRepository
    + DependOnRegistrationEnforcementProbeClient
{
    /// Check persisted business evidence under the gate lock. A provider update cannot heal it.
    fn refresh_registration_enforcement(
        &self,
    ) -> impl Future<Output = error_stack::Result<RegistrationGateView, KernelError>> + Send {
        async move {
            let deps = self.clone();
            let state = self
                .transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        let mut observation = deps
                            .registration_gate_repository()
                            .load_for_update(executor)
                            .await?
                            .observation;
                        let evidence = deps
                            .registration_enforcement_probe_repository()
                            .evidence(executor)
                            .await?;
                        observation.enforcement_verified = evidence.is_some_and(|e| {
                            !deps
                                .registration_enforcement_probe_client()
                                .target_keys()
                                .is_empty()
                                && e.is_current(
                                    deps.registration_enforcement_probe_client().generation(),
                                    OffsetDateTime::now_utc(),
                                )
                        });
                        apply_gate_observation(
                            deps.registration_gate_repository(),
                            executor,
                            observation,
                            OffsetDateTime::now_utc(),
                        )
                        .await
                    })
                })
                .await?;
            Ok(RegistrationGateView::new(state))
        }
    }

    /// This acknowledges only a pre-created random probe bound to its real flow.
    /// Every reserved canary is denied, including malformed/unregistered probes.
    fn receive_registration_enforcement_canary(
        &self,
        id: Option<&str>,
        flow_id: &str,
        email: &str,
    ) -> impl Future<Output = error_stack::Result<bool, KernelError>> + Send {
        async move {
            let Some(id) = id.and_then(EnforcementProbeId::parse) else {
                return Ok(false);
            };
            if id.email() != email.to_ascii_lowercase() {
                return Ok(false);
            }
            let mut executor = self.database_connection().connection().await?;
            self.registration_enforcement_probe_repository()
                .receive(&mut executor, &id, flow_id, OffsetDateTime::now_utc())
                .await
        }
    }

    fn invalidate_registration_enforcement(
        &self,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        async move {
            // Persist stop/notification before cleanup or any further external I/O.
            let deps = self.clone();
            self.transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        let at = OffsetDateTime::now_utc();
                        let mut observation = deps
                            .registration_gate_repository()
                            .load_for_update(executor)
                            .await?
                            .observation;
                        deps.registration_enforcement_probe_repository()
                            .save_evidence(
                                executor,
                                &EnforcementProbeEvidence {
                                    generation: deps
                                        .registration_enforcement_probe_client()
                                        .generation()
                                        .to_string(),
                                    verified: false,
                                    checked_at: at,
                                    valid_until: at,
                                },
                            )
                            .await?;
                        observation.enforcement_verified = false;
                        apply_gate_observation(
                            deps.registration_gate_repository(),
                            executor,
                            observation,
                            at,
                        )
                        .await?;
                        Ok(())
                    })
                })
                .await
        }
    }

    fn run_registration_enforcement_probe(
        &self,
        interval: time::Duration,
    ) -> impl Future<Output = error_stack::Result<bool, KernelError>> + Send {
        async move {
            if interval < time::Duration::seconds(1) || interval > time::Duration::minutes(5) {
                return Err(Report::new(KernelError::Validation));
            }
            let deps = self.clone();
            self.transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        let began = OffsetDateTime::now_utc();
                        if !deps
                            .registration_enforcement_probe_repository()
                            .begin_cycle(executor, began, interval)
                            .await?
                        {
                            return Ok(false);
                        }
                        let client = deps.registration_enforcement_probe_client();
                        let targets = client.target_keys();
                        let mut verified = !targets.is_empty();
                        let mut connection = deps.database_connection().connection().await?;
                        let pending = deps
                            .registration_enforcement_probe_repository()
                            .pending_cleanup(&mut connection, began)
                            .await?;
                        drop(connection);
                        if pending.len() > 512 {
                            deps.invalidate_registration_enforcement().await?;
                            return Err(Report::new(KernelError::Internal));
                        }
                        for canary in pending {
                            if !cleanup(&deps, &canary).await? {
                                verified = false;
                            }
                        }
                        for target in targets {
                            for mode in [EnforcementProbeMode::Api, EnforcementProbeMode::Browser] {
                                let flow = match client.initialize(&target, mode).await {
                                    Ok(flow) => flow,
                                    Err(_) => {
                                        deps.invalidate_registration_enforcement().await?;
                                        verified = false;
                                        continue;
                                    }
                                };
                                let canary = EnforcementCanary {
                                    id: probe_id(),
                                    target_key: target.clone(),
                                    flow_id: flow.flow_id.clone(),
                                    created_at: OffsetDateTime::now_utc(),
                                };
                                let mut connection =
                                    deps.database_connection().connection().await?;
                                deps.registration_enforcement_probe_repository()
                                    .prepare(&mut connection, &canary)
                                    .await?;
                                drop(connection);
                                let denied =
                                    client.submit(&flow, &canary.id).await.unwrap_or(false);
                                if !denied {
                                    deps.invalidate_registration_enforcement().await?;
                                }
                                let mut connection =
                                    deps.database_connection().connection().await?;
                                let received = deps
                                    .registration_enforcement_probe_repository()
                                    .was_received(&mut connection, &canary.id)
                                    .await?;
                                drop(connection);
                                if !received {
                                    deps.invalidate_registration_enforcement().await?;
                                }
                                let clear = cleanup(&deps, &canary).await?;
                                let confirmed = denied && received && clear;
                                verified &= confirmed;
                                if confirmed {
                                    let mut connection =
                                        deps.database_connection().connection().await?;
                                    deps.registration_enforcement_probe_repository()
                                        .settle(&mut connection, &canary.id)
                                        .await?;
                                }
                            }
                        }
                        let mut connection = deps.database_connection().connection().await?;
                        deps.registration_enforcement_probe_repository()
                            .save_evidence(
                                &mut connection,
                                &EnforcementProbeEvidence {
                                    generation: client.generation().to_string(),
                                    verified,
                                    checked_at: began,
                                    valid_until: began + interval * 2,
                                },
                            )
                            .await?;
                        drop(connection);
                        deps.refresh_registration_enforcement().await?;
                        Ok(true)
                    })
                })
                .await
        }
    }
}

async fn cleanup<T: RegistrationEnforcementMonitorUseCase>(
    deps: &T,
    canary: &EnforcementCanary,
) -> error_stack::Result<bool, KernelError> {
    let client = deps.registration_enforcement_probe_client();
    let identities = match client
        .created_identities(&canary.target_key, &canary.id)
        .await
    {
        Ok(ids) => ids,
        Err(_) => {
            deps.invalidate_registration_enforcement().await?;
            return Err(Report::new(KernelError::Internal));
        }
    };
    if identities.is_empty() {
        return Ok(true);
    }
    deps.invalidate_registration_enforcement().await?;
    for identity_id in identities {
        client
            .delete_identity(&canary.target_key, &canary.id, &identity_id)
            .await?;
    }
    Ok(false)
}

impl<T> RegistrationEnforcementMonitorUseCase for T where
    T: RecordRegistrationEnforcementUseCase
        + DependOnRegistrationEnforcementProbeRepository
        + DependOnRegistrationEnforcementProbeClient
{
}
