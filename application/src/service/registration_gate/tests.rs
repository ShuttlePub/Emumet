use super::*;
use kernel::interfaces::database::{
    Connection, DatabaseConnection, DependOnDatabaseConnection, TransactionManager,
};
use kernel::interfaces::permission::{
    DependOnPermissionChecker, InstanceRole, PermissionChecker, PermissionReq,
};
use kernel::interfaces::repository::{
    DependOnRegistrationGateRepository, RegistrationGateRepository,
};
use kernel::prelude::entity::{
    AdminState, GateAction, GateTransitionRecord, ProbeStatus, ProtectionObservation,
    RegistrationGateAudit, RegistrationGateEvent,
};
use std::collections::HashSet;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct FakeConnection;

impl Connection for FakeConnection {}

#[derive(Clone)]
struct FakeDatabase;

impl DatabaseConnection for FakeDatabase {
    type Connection = FakeConnection;

    async fn connection(&self) -> error_stack::Result<Self::Connection, KernelError> {
        Ok(FakeConnection)
    }
}

impl TransactionManager for FakeDatabase {
    fn transaction<'a, F, T>(
        &'a self,
        operation: F,
    ) -> Pin<Box<dyn Future<Output = error_stack::Result<T, KernelError>> + Send + 'a>>
    where
        F: for<'connection> FnOnce(
                &'connection mut Self::Connection,
            ) -> Pin<
                Box<dyn Future<Output = error_stack::Result<T, KernelError>> + Send + 'connection>,
            > + Send
            + 'a,
        T: Send + 'a,
    {
        Box::pin(async move { operation(&mut FakeConnection).await })
    }
}

#[derive(Default)]
struct Store {
    state: Option<RegistrationGateState>,
    audit: Vec<RegistrationGateAudit>,
    events: Vec<RegistrationGateEvent>,
    reads: HashSet<(i64, i64)>,
}

#[derive(Clone, Default)]
struct FakeRepository {
    store: Arc<Mutex<Store>>,
}

impl FakeRepository {
    fn current(&self) -> RegistrationGateState {
        let mut store = self.store.lock().unwrap();
        store
            .state
            .get_or_insert_with(|| RegistrationGateState::initial(OffsetDateTime::UNIX_EPOCH))
            .clone()
    }
}

impl RegistrationGateRepository for FakeRepository {
    type Connection = FakeConnection;

    async fn load(
        &self,
        _executor: &mut Self::Connection,
    ) -> error_stack::Result<RegistrationGateState, KernelError> {
        Ok(self.current())
    }

    async fn load_for_update(
        &self,
        _executor: &mut Self::Connection,
    ) -> error_stack::Result<RegistrationGateState, KernelError> {
        Ok(self.current())
    }

    async fn save_transition(
        &self,
        _executor: &mut Self::Connection,
        record: &GateTransitionRecord,
    ) -> error_stack::Result<(), KernelError> {
        let current = self.current();
        if current.revision != record.expected_revision {
            return Err(Report::new(KernelError::Concurrency));
        }
        let mut store = self.store.lock().unwrap();
        store.state = Some(record.state.clone());
        store.audit.push(record.audit.clone());
        store.events.extend(record.events.clone());
        Ok(())
    }

    async fn save_observation(
        &self,
        _executor: &mut Self::Connection,
        observation: &RegistrationObservation,
        _updated_at: OffsetDateTime,
    ) -> error_stack::Result<(), KernelError> {
        let mut state = self.current();
        state.observation = *observation;
        self.store.lock().unwrap().state = Some(state);
        Ok(())
    }

    async fn list_audit(
        &self,
        _executor: &mut Self::Connection,
        _limit: i64,
    ) -> error_stack::Result<Vec<RegistrationGateAudit>, KernelError> {
        Ok(self.store.lock().unwrap().audit.clone())
    }

    async fn list_notifications(
        &self,
        _executor: &mut Self::Connection,
        admin: &AuthAccountId,
        before: Option<&RegistrationGateEventId>,
        limit: i64,
    ) -> error_stack::Result<Vec<RegistrationGateNotification>, KernelError> {
        let store = self.store.lock().unwrap();
        let mut events: Vec<_> = store
            .events
            .iter()
            .filter(|e| before.is_none_or(|b| e.id.as_ref() < b.as_ref()))
            .cloned()
            .collect();
        events.sort_by_key(|e| std::cmp::Reverse(*e.id.as_ref()));
        Ok(events
            .into_iter()
            .take(limit as usize)
            .map(|event| {
                let read = store.reads.contains(&(*admin.as_ref(), *event.id.as_ref()));
                RegistrationGateNotification {
                    read_at: read.then_some(OffsetDateTime::UNIX_EPOCH),
                    event,
                }
            })
            .collect())
    }

    async fn count_unread(
        &self,
        _executor: &mut Self::Connection,
        admin: &AuthAccountId,
    ) -> error_stack::Result<i64, KernelError> {
        let store = self.store.lock().unwrap();
        Ok(store
            .events
            .iter()
            .filter(|e| !store.reads.contains(&(*admin.as_ref(), *e.id.as_ref())))
            .count() as i64)
    }

    async fn mark_read(
        &self,
        _executor: &mut Self::Connection,
        admin: &AuthAccountId,
        event_ids: &[RegistrationGateEventId],
        _read_at: OffsetDateTime,
    ) -> error_stack::Result<u64, KernelError> {
        let mut store = self.store.lock().unwrap();
        let known: Vec<i64> = store.events.iter().map(|e| *e.id.as_ref()).collect();
        let mut marked = 0;
        for id in event_ids.iter().filter(|id| known.contains(id.as_ref())) {
            if store.reads.insert((*admin.as_ref(), *id.as_ref())) {
                marked += 1;
            }
        }
        Ok(marked)
    }

    async fn mark_all_read(
        &self,
        _executor: &mut Self::Connection,
        admin: &AuthAccountId,
        _read_at: OffsetDateTime,
    ) -> error_stack::Result<u64, KernelError> {
        let mut store = self.store.lock().unwrap();
        let ids: Vec<i64> = store.events.iter().map(|e| *e.id.as_ref()).collect();
        let mut marked = 0;
        for id in ids {
            if store.reads.insert((*admin.as_ref(), id)) {
                marked += 1;
            }
        }
        Ok(marked)
    }
}

#[derive(Clone)]
struct FakePermissionChecker {
    admins: Arc<Mutex<Vec<AuthAccountId>>>,
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
        Ok(self.admins.lock().unwrap().contains(subject))
    }

    async fn list_instance_roles(
        &self,
        _subject: &AuthAccountId,
    ) -> error_stack::Result<Vec<InstanceRole>, KernelError> {
        Ok(Vec::new())
    }
}

#[derive(Clone)]
struct MockModule {
    database: FakeDatabase,
    repository: FakeRepository,
    permission_checker: FakePermissionChecker,
}

impl DependOnDatabaseConnection for MockModule {
    type DatabaseConnection = FakeDatabase;

    fn database_connection(&self) -> &Self::DatabaseConnection {
        &self.database
    }
}

impl kernel::interfaces::database::DependOnTransactionManager for MockModule {
    type TransactionManager = FakeDatabase;

    fn transaction_manager(&self) -> &Self::TransactionManager {
        &self.database
    }
}

impl DependOnRegistrationGateRepository for MockModule {
    type RegistrationGateRepository = FakeRepository;

    fn registration_gate_repository(&self) -> &Self::RegistrationGateRepository {
        &self.repository
    }
}

impl DependOnPermissionChecker for MockModule {
    type PermissionChecker = FakePermissionChecker;

    fn permission_checker(&self) -> &Self::PermissionChecker {
        &self.permission_checker
    }
}

struct Fixture {
    module: MockModule,
    admin: AuthAccountId,
    other_admin: AuthAccountId,
    outsider: AuthAccountId,
}

fn fixture() -> Fixture {
    let admin = AuthAccountId::new(1);
    let other_admin = AuthAccountId::new(2);
    Fixture {
        module: MockModule {
            database: FakeDatabase,
            repository: FakeRepository::default(),
            permission_checker: FakePermissionChecker {
                admins: Arc::new(Mutex::new(vec![admin.clone(), other_admin.clone()])),
                checked: Arc::new(Mutex::new(Vec::new())),
            },
        },
        admin,
        other_admin,
        outsider: AuthAccountId::new(3),
    }
}

const H: ProtectionObservation = ProtectionObservation {
    configured: true,
    probe: ProbeStatus::Healthy,
};
const DOWN: ProtectionObservation = ProtectionObservation {
    configured: true,
    probe: ProbeStatus::Unhealthy,
};
const OFF: ProtectionObservation = ProtectionObservation::UNCONFIGURED;

fn obs(email: ProtectionObservation, bot: ProtectionObservation) -> RegistrationObservation {
    RegistrationObservation::new(email, bot)
}

fn kind<T: std::fmt::Debug>(result: error_stack::Result<T, KernelError>) -> KernelError {
    match result.expect_err("expected error").current_context() {
        KernelError::PermissionDenied => KernelError::PermissionDenied,
        KernelError::Concurrency => KernelError::Concurrency,
        KernelError::Rejected => KernelError::Rejected,
        KernelError::Validation => KernelError::Validation,
        other => panic!("unexpected error {other:?}"),
    }
}

#[tokio::test]
async fn every_admin_use_case_rejects_non_admin_without_touching_the_gate() {
    kernel::ensure_generator_initialized();
    let fx = fixture();
    let m = &fx.module;
    let who = &fx.outsider;

    assert_eq!(
        kind(m.get_registration_gate(who).await),
        KernelError::PermissionDenied
    );
    assert_eq!(
        kind(m.open_registration_gate(who, 0).await),
        KernelError::PermissionDenied
    );
    assert_eq!(
        kind(m.close_registration_gate(who, 0).await),
        KernelError::PermissionDenied
    );
    assert_eq!(
        kind(m.enable_registration_gate_override(who, 0, true).await),
        KernelError::PermissionDenied
    );
    assert_eq!(
        kind(m.revoke_registration_gate_override(who, 0).await),
        KernelError::PermissionDenied
    );
    assert_eq!(
        kind(
            m.list_registration_gate_notifications(who, None, None)
                .await
        ),
        KernelError::PermissionDenied
    );
    assert_eq!(
        kind(m.count_unread_registration_gate_notifications(who).await),
        KernelError::PermissionDenied
    );
    assert_eq!(
        kind(
            m.mark_registration_gate_notifications_read(who, vec![RegistrationGateEventId::new(1)])
                .await
        ),
        KernelError::PermissionDenied
    );
    assert_eq!(
        kind(m.mark_all_registration_gate_notifications_read(who).await),
        KernelError::PermissionDenied
    );

    let store = m.repository.store.lock().unwrap();
    assert!(store.audit.is_empty() && store.events.is_empty());
    assert_eq!(store.state.as_ref().map_or(0, |s| s.revision), 0);
    drop(store);
    let checked = m.permission_checker.checked.lock().unwrap();
    assert!(checked
        .iter()
        .all(|c| c == &("Instance".to_string(), "administrate")));
    assert_eq!(checked.len(), 9);
}

#[tokio::test]
async fn admin_open_requires_minimum_condition_then_succeeds_and_close_is_manual() {
    kernel::ensure_generator_initialized();
    let fx = fixture();
    let m = &fx.module;

    let initial = m.get_registration_gate(&fx.admin).await.unwrap();
    assert_eq!(initial.state.admin, AdminState::Closed);
    assert_eq!(initial.decision, RegistrationDecision::AdminClosed);

    assert_eq!(
        kind(m.open_registration_gate(&fx.admin, 0).await),
        KernelError::Rejected
    );

    m.record_registration_observation(obs(H, OFF))
        .await
        .unwrap();
    let opened = m.open_registration_gate(&fx.admin, 0).await.unwrap();
    assert!(opened.decision.accepts());
    assert_eq!(opened.state.revision, 1);

    let closed = m
        .close_registration_gate(&fx.admin, opened.state.revision)
        .await
        .unwrap();
    assert_eq!(closed.decision, RegistrationDecision::AdminClosed);
    m.record_registration_observation(obs(H, H)).await.unwrap();
    assert_eq!(
        m.get_registration_gate(&fx.admin).await.unwrap().decision,
        RegistrationDecision::AdminClosed,
        "recovery never reopens a manual close"
    );
}

#[tokio::test]
async fn stale_revision_is_a_concurrency_conflict() {
    kernel::ensure_generator_initialized();
    let fx = fixture();
    let m = &fx.module;
    m.enable_registration_gate_override(&fx.admin, 0, true)
        .await
        .unwrap();

    assert_eq!(
        kind(m.close_registration_gate(&fx.other_admin, 0).await),
        KernelError::Concurrency
    );
}

#[tokio::test]
async fn override_requires_warning_ack_and_is_visible_with_activator() {
    kernel::ensure_generator_initialized();
    let fx = fixture();
    let m = &fx.module;

    assert_eq!(
        kind(
            m.enable_registration_gate_override(&fx.admin, 0, false)
                .await
        ),
        KernelError::Validation
    );
    let view = m
        .enable_registration_gate_override(&fx.admin, 0, true)
        .await
        .unwrap();
    assert!(view.decision.accepts());
    assert_eq!(view.state.override_info.unwrap().activated_by, fx.admin);

    let store = m.repository.store.lock().unwrap();
    assert_eq!(store.audit.len(), 1);
    assert_eq!(store.audit[0].action, GateAction::EnableOverride);
    assert_eq!(store.events.len(), 1);
}

#[tokio::test]
async fn observation_use_case_needs_no_admin_and_repetition_does_not_multiply_events() {
    kernel::ensure_generator_initialized();
    let fx = fixture();
    let m = &fx.module;
    m.record_registration_observation(obs(H, H)).await.unwrap();
    m.open_registration_gate(&fx.admin, 0).await.unwrap();

    for _ in 0..5 {
        m.record_registration_observation(obs(DOWN, H))
            .await
            .unwrap();
    }

    let store = m.repository.store.lock().unwrap();
    assert_eq!(store.events.len(), 1);
    assert_eq!(store.audit.len(), 2, "open + one observation transition");
    assert_eq!(store.state.as_ref().unwrap().revision, 2);
}

#[tokio::test]
async fn notification_read_state_is_per_admin_and_ids_are_validated() {
    kernel::ensure_generator_initialized();
    let fx = fixture();
    let m = &fx.module;
    m.record_registration_observation(obs(H, H)).await.unwrap();
    m.open_registration_gate(&fx.admin, 0).await.unwrap();
    m.record_registration_observation(obs(DOWN, DOWN))
        .await
        .unwrap();
    m.record_registration_observation(obs(H, H)).await.unwrap();

    let listed = m
        .list_registration_gate_notifications(&fx.admin, None, None)
        .await
        .unwrap();
    assert_eq!(listed.notifications.len(), 2);
    assert_eq!(listed.unread_count, 2);

    let left = m
        .mark_registration_gate_notifications_read(
            &fx.admin,
            vec![listed.notifications[0].event.id.clone()],
        )
        .await
        .unwrap();
    assert_eq!(left, 1);
    assert_eq!(
        m.count_unread_registration_gate_notifications(&fx.other_admin)
            .await
            .unwrap(),
        2,
        "other admin's unread count is independent"
    );
    let other_view = m
        .list_registration_gate_notifications(&fx.other_admin, None, None)
        .await
        .unwrap();
    assert!(other_view.notifications.iter().all(|n| n.read_at.is_none()));

    assert_eq!(
        kind(
            m.mark_registration_gate_notifications_read(&fx.admin, vec![])
                .await
        ),
        KernelError::Validation
    );
    assert_eq!(
        kind(
            m.mark_registration_gate_notifications_read(
                &fx.admin,
                (0..=MAX_MARK_READ_IDS as i64)
                    .map(RegistrationGateEventId::new)
                    .collect()
            )
            .await
        ),
        KernelError::Validation
    );
    assert_eq!(
        m.mark_all_registration_gate_notifications_read(&fx.other_admin)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        m.count_unread_registration_gate_notifications(&fx.admin)
            .await
            .unwrap(),
        1
    );
}
