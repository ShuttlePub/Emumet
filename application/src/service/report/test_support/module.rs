use super::account::MockAccountQuery;
use super::database::MockDatabaseConnection;
use super::report::{MockAccountReportReadModel, MockAccountReportRepository};
use kernel::interfaces::database::{DependOnDatabaseConnection, DependOnTransactionManager};
use kernel::interfaces::permission::{
    DependOnPermissionChecker, InstanceRole, PermissionChecker, PermissionReq,
};
use kernel::interfaces::read_model::{DependOnAccountQuery, DependOnAccountReportReadModel};
use kernel::interfaces::repository::DependOnAccountReportRepository;
use kernel::prelude::entity::{
    Account, AccountId, AccountReport, AccountReportEvent, AuthAccountId, Nanoid,
};
use kernel::test_utils::AccountBuilder;
use kernel::KernelError;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct MockPermissionChecker {
    allowed: bool,
}

impl PermissionChecker for MockPermissionChecker {
    async fn check(
        &self,
        _subject: &AuthAccountId,
        _req: &PermissionReq,
    ) -> error_stack::Result<bool, KernelError> {
        Ok(self.allowed)
    }

    async fn list_instance_roles(
        &self,
        _subject: &AuthAccountId,
    ) -> error_stack::Result<Vec<InstanceRole>, KernelError> {
        Ok(Vec::new())
    }
}

#[derive(Clone)]
pub struct MockModule {
    database: MockDatabaseConnection,
    accounts: MockAccountQuery,
    report_repository: MockAccountReportRepository,
    reports: MockAccountReportReadModel,
    permission_checker: MockPermissionChecker,
}

impl DependOnDatabaseConnection for MockModule {
    type DatabaseConnection = MockDatabaseConnection;

    fn database_connection(&self) -> &Self::DatabaseConnection {
        &self.database
    }
}

impl DependOnTransactionManager for MockModule {
    type TransactionManager = MockDatabaseConnection;

    fn transaction_manager(&self) -> &Self::TransactionManager {
        &self.database
    }
}

impl DependOnAccountQuery for MockModule {
    type AccountQuery = MockAccountQuery;

    fn account_query(&self) -> &Self::AccountQuery {
        &self.accounts
    }
}

impl DependOnAccountReportRepository for MockModule {
    type AccountReportRepository = MockAccountReportRepository;

    fn account_report_repository(&self) -> &Self::AccountReportRepository {
        &self.report_repository
    }
}

impl DependOnAccountReportReadModel for MockModule {
    type AccountReportReadModel = MockAccountReportReadModel;

    fn account_report_read_model(&self) -> &Self::AccountReportReadModel {
        &self.reports
    }
}

impl DependOnPermissionChecker for MockModule {
    type PermissionChecker = MockPermissionChecker;

    fn permission_checker(&self) -> &Self::PermissionChecker {
        &self.permission_checker
    }
}

pub struct Fixture {
    pub module: MockModule,
    pub operator_id: AuthAccountId,
    pub target_nanoid: String,
}

pub fn fixture(
    owned_account: bool,
    target_exists: bool,
    allowed: bool,
    report: Option<AccountReport>,
    projections: Vec<kernel::interfaces::read_model::AccountReportProjection>,
) -> Fixture {
    kernel::ensure_generator_initialized();
    let reporter = AccountBuilder::new().id(AccountId::new(10)).build();
    let target_nanoid = "target-account".to_string();
    let target = AccountBuilder::new()
        .id(AccountId::new(20))
        .nanoid(Nanoid::<Account>::new(target_nanoid.clone()))
        .build();
    Fixture {
        module: MockModule {
            database: MockDatabaseConnection,
            accounts: MockAccountQuery {
                owned_accounts: if owned_account {
                    vec![reporter]
                } else {
                    Vec::new()
                },
                target: target_exists.then_some(target),
            },
            report_repository: MockAccountReportRepository {
                report,
                saved_events: Arc::new(Mutex::new(Vec::new())),
            },
            reports: MockAccountReportReadModel {
                reports: Arc::new(Mutex::new(projections)),
            },
            permission_checker: MockPermissionChecker { allowed },
        },
        operator_id: AuthAccountId::default(),
        target_nanoid,
    }
}

pub fn saved_events(fixture: &Fixture) -> Vec<AccountReportEvent> {
    fixture
        .module
        .report_repository
        .saved_events
        .lock()
        .unwrap()
        .clone()
}
