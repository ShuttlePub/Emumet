use super::*;
use kernel::interfaces::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::permission::{
    DependOnPermissionChecker, InstanceRole, PermissionChecker, PermissionReq,
};
use kernel::interfaces::repository::{
    DependOnWebhookDeliveryRepository, WebhookIngestReport, WebhookRequest,
};
use kernel::prelude::entity::{
    ClaimedWebhookDelivery, WebhookAttempt, WebhookDecision, WebhookResultClass,
    WebhookSendObservation,
};
use std::sync::{Arc, Mutex};

struct FakeConnection;

impl Connection for FakeConnection {}

struct FakeDatabase;

impl DatabaseConnection for FakeDatabase {
    type Connection = FakeConnection;

    async fn connection(&self) -> error_stack::Result<Self::Connection, KernelError> {
        Ok(FakeConnection)
    }
}

#[derive(Default)]
struct RepositoryLog {
    retried: Vec<Option<i64>>,
    reads: usize,
}

#[derive(Clone, Default)]
struct FakeRepository {
    failed: Arc<Mutex<Vec<i64>>>,
    log: Arc<Mutex<RepositoryLog>>,
}

impl WebhookDeliveryRepository for FakeRepository {
    type Connection = FakeConnection;

    async fn ingest_gate_events(
        &self,
        _executor: &mut Self::Connection,
        _now: OffsetDateTime,
        _limit: i64,
    ) -> error_stack::Result<WebhookIngestReport, KernelError> {
        Ok(WebhookIngestReport {
            baseline_event_id: 0,
            ingested: 0,
        })
    }

    async fn claim_due(
        &self,
        _executor: &mut Self::Connection,
        _owner: &str,
        _now: OffsetDateTime,
        _policy: &WebhookDeliveryPolicy,
    ) -> error_stack::Result<Vec<ClaimedWebhookDelivery>, KernelError> {
        Ok(Vec::new())
    }

    async fn record_attempt(
        &self,
        _executor: &mut Self::Connection,
        _claim: &ClaimedWebhookDelivery,
        _owner: &str,
        _finished_at: OffsetDateTime,
        _decision: &WebhookDecision,
    ) -> error_stack::Result<bool, KernelError> {
        Ok(true)
    }

    async fn retry_failed(
        &self,
        _executor: &mut Self::Connection,
        event_id: Option<&RegistrationGateEventId>,
        _now: OffsetDateTime,
    ) -> error_stack::Result<u64, KernelError> {
        self.log
            .lock()
            .unwrap()
            .retried
            .push(event_id.map(|id| *id.as_ref()));
        let mut failed = self.failed.lock().unwrap();
        let before = failed.len();
        match event_id {
            Some(id) => failed.retain(|failed| failed != id.as_ref()),
            None => failed.clear(),
        }
        Ok((before - failed.len()) as u64)
    }

    async fn summary(
        &self,
        _executor: &mut Self::Connection,
    ) -> error_stack::Result<WebhookDeliverySummary, KernelError> {
        self.log.lock().unwrap().reads += 1;
        let mut summary = WebhookDeliverySummary::default();
        summary.counts.failed = self.failed.lock().unwrap().len() as i64;
        Ok(summary)
    }

    async fn list_recent(
        &self,
        _executor: &mut Self::Connection,
        _limit: i64,
    ) -> error_stack::Result<Vec<WebhookDelivery>, KernelError> {
        self.log.lock().unwrap().reads += 1;
        Ok(Vec::new())
    }

    async fn list_attempts(
        &self,
        _executor: &mut Self::Connection,
        _event_id: &RegistrationGateEventId,
    ) -> error_stack::Result<Vec<WebhookAttempt>, KernelError> {
        Ok(Vec::new())
    }
}

#[derive(Clone)]
struct FakeSender {
    status: WebhookConfigStatus,
    observation: WebhookSendObservation,
    sent: Arc<Mutex<Vec<(String, String, u32)>>>,
}

impl WebhookSender for FakeSender {
    fn status(&self) -> WebhookConfigStatus {
        self.status.clone()
    }

    async fn send(&self, request: &WebhookRequest<'_>) -> WebhookSendObservation {
        self.sent.lock().unwrap().push((
            request.event_id.to_string(),
            request.event_type.as_str().to_string(),
            request.attempt,
        ));
        self.observation.clone()
    }
}

#[derive(Clone)]
struct FakePermissionChecker {
    admins: Vec<AuthAccountId>,
    checked: Arc<Mutex<Vec<(String, &'static str)>>>,
}

impl PermissionChecker for FakePermissionChecker {
    async fn check(
        &self,
        subject: &AuthAccountId,
        req: &PermissionReq,
    ) -> error_stack::Result<bool, KernelError> {
        self.checked
            .lock()
            .unwrap()
            .push((req.namespace().to_string(), req.permission_name()));
        Ok(self.admins.contains(subject))
    }

    async fn list_instance_roles(
        &self,
        _subject: &AuthAccountId,
    ) -> error_stack::Result<Vec<InstanceRole>, KernelError> {
        Ok(Vec::new())
    }
}

struct MockModule {
    database: FakeDatabase,
    repository: FakeRepository,
    sender: FakeSender,
    permission_checker: FakePermissionChecker,
}

impl DependOnDatabaseConnection for MockModule {
    type DatabaseConnection = FakeDatabase;

    fn database_connection(&self) -> &Self::DatabaseConnection {
        &self.database
    }
}

impl DependOnWebhookDeliveryRepository for MockModule {
    type WebhookDeliveryRepository = FakeRepository;

    fn webhook_delivery_repository(&self) -> &Self::WebhookDeliveryRepository {
        &self.repository
    }
}

impl DependOnWebhookSender for MockModule {
    type WebhookSender = FakeSender;

    fn webhook_sender(&self) -> &Self::WebhookSender {
        &self.sender
    }
}

impl DependOnPermissionChecker for MockModule {
    type PermissionChecker = FakePermissionChecker;

    fn permission_checker(&self) -> &Self::PermissionChecker {
        &self.permission_checker
    }
}

fn module(status: WebhookConfigStatus, observation: WebhookSendObservation) -> MockModule {
    MockModule {
        database: FakeDatabase,
        repository: FakeRepository::default(),
        sender: FakeSender {
            status,
            observation,
            sent: Arc::default(),
        },
        permission_checker: FakePermissionChecker {
            admins: vec![AuthAccountId::new(1)],
            checked: Arc::default(),
        },
    }
}

fn enabled() -> WebhookConfigStatus {
    WebhookConfigStatus::Enabled {
        host: "hooks.example.com".into(),
    }
}

fn ok() -> WebhookSendObservation {
    WebhookSendObservation::Response {
        status: 204,
        retry_after: None,
    }
}

fn error_kind<T: std::fmt::Debug>(result: error_stack::Result<T, KernelError>) -> KernelError {
    match result.expect_err("expected error").current_context() {
        KernelError::PermissionDenied => KernelError::PermissionDenied,
        KernelError::NotFound => KernelError::NotFound,
        other => panic!("unexpected error {other:?}"),
    }
}

#[tokio::test]
async fn every_admin_use_case_rejects_non_admin_before_touching_sender_or_repository() {
    kernel::ensure_generator_initialized();
    let m = module(enabled(), ok());
    let outsider = AuthAccountId::new(2);
    m.repository.failed.lock().unwrap().push(7);

    assert_eq!(
        error_kind(m.get_registration_gate_webhook_status(&outsider).await),
        KernelError::PermissionDenied
    );
    assert_eq!(
        error_kind(m.send_registration_gate_webhook_test(&outsider).await),
        KernelError::PermissionDenied
    );
    assert_eq!(
        error_kind(
            m.retry_failed_registration_gate_webhooks(&outsider, None)
                .await
        ),
        KernelError::PermissionDenied
    );
    assert!(m.sender.sent.lock().unwrap().is_empty());
    let log = m.repository.log.lock().unwrap();
    assert!(log.retried.is_empty());
    assert_eq!(log.reads, 0);
    assert_eq!(m.repository.failed.lock().unwrap().len(), 1);
    assert!(m
        .permission_checker
        .checked
        .lock()
        .unwrap()
        .iter()
        .all(|(namespace, permission)| namespace == "Instance" && *permission == "administrate"));
}

#[tokio::test]
async fn status_reports_configuration_and_counts_for_admins_without_secrets() {
    let m = module(enabled(), ok());
    m.repository.failed.lock().unwrap().extend([1, 2]);
    let admin = AuthAccountId::new(1);

    let status = m
        .get_registration_gate_webhook_status(&admin)
        .await
        .unwrap();
    assert_eq!(status.config, enabled());
    assert_eq!(status.summary.counts.failed, 2);

    for config in [
        WebhookConfigStatus::Disabled,
        WebhookConfigStatus::Misconfigured {
            issue: kernel::prelude::entity::WebhookConfigIssue::MissingSecret,
        },
    ] {
        let m = module(config.clone(), ok());
        assert_eq!(
            m.get_registration_gate_webhook_status(&admin)
                .await
                .unwrap()
                .config,
            config
        );
    }
}

#[tokio::test]
async fn test_send_goes_through_the_sender_once_and_reports_the_result() {
    kernel::ensure_generator_initialized();
    let admin = AuthAccountId::new(1);

    let m = module(enabled(), ok());
    let outcome = m.send_registration_gate_webhook_test(&admin).await.unwrap();
    let WebhookTestOutcome::Sent {
        event_id,
        delivered,
        result,
        http_status,
        ..
    } = outcome
    else {
        panic!("expected a sent test")
    };
    assert!(delivered);
    assert_eq!(result, WebhookResultClass::Delivered);
    assert_eq!(http_status, Some(204));
    assert_eq!(
        m.sender.sent.lock().unwrap().as_slice(),
        &[(event_id, "webhook_test".to_string(), 1)]
    );
    assert_eq!(m.repository.log.lock().unwrap().reads, 0);

    let m = module(
        enabled(),
        WebhookSendObservation::Response {
            status: 500,
            retry_after: None,
        },
    );
    let WebhookTestOutcome::Sent {
        delivered, result, ..
    } = m.send_registration_gate_webhook_test(&admin).await.unwrap()
    else {
        panic!("expected a sent test")
    };
    assert!(!delivered);
    assert_eq!(result, WebhookResultClass::HttpServerError);

    let m = module(WebhookConfigStatus::Disabled, ok());
    assert_eq!(
        m.send_registration_gate_webhook_test(&admin).await.unwrap(),
        WebhookTestOutcome::NotConfigured
    );
    assert!(m.sender.sent.lock().unwrap().is_empty());
}

#[tokio::test]
async fn retry_requeues_failed_deliveries_and_unknown_event_is_not_found() {
    let m = module(enabled(), ok());
    m.repository.failed.lock().unwrap().extend([10, 11, 12]);
    let admin = AuthAccountId::new(1);

    assert_eq!(
        m.retry_failed_registration_gate_webhooks(&admin, Some(RegistrationGateEventId::new(11)))
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        error_kind(
            m.retry_failed_registration_gate_webhooks(
                &admin,
                Some(RegistrationGateEventId::new(11))
            )
            .await
        ),
        KernelError::NotFound
    );
    assert_eq!(
        m.retry_failed_registration_gate_webhooks(&admin, None)
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        m.retry_failed_registration_gate_webhooks(&admin, None)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn worker_cycle_does_nothing_when_the_webhook_is_not_enabled() {
    for config in [
        WebhookConfigStatus::Disabled,
        WebhookConfigStatus::Misconfigured {
            issue: kernel::prelude::entity::WebhookConfigIssue::InvalidUrl,
        },
    ] {
        let m = module(config, ok());
        let jitter = || 0.5;
        let stop = || false;
        let report = m
            .deliver_registration_gate_webhooks(
                "worker",
                &WebhookDeliveryPolicy::default(),
                &jitter,
                &stop,
            )
            .await
            .unwrap();
        assert!(report.skipped);
        assert!(m.sender.sent.lock().unwrap().is_empty());
        assert_eq!(m.repository.log.lock().unwrap().reads, 0);
    }
}
