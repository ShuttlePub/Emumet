use super::*;
use error_stack::Report;
use kernel::interfaces::database::{
    Connection, DatabaseConnection, DependOnDatabaseConnection, DependOnTransactionManager,
    TransactionManager,
};
use kernel::interfaces::read_model::{AccountQuery, AccountReadModel, DependOnAccountReadModel};
use kernel::interfaces::repository::{
    AggregateRepository, DependOnAccountRepository, DependOnOrganizationMembershipRepository,
    OrganizationMembershipRepository, Rehydrated,
};
use kernel::prelude::entity::{
    Account, AccountEvent, AccountId, AccountKind, AccountName, AuthAccountId, CommandEnvelope,
    CreatedAt, EventEnvelope, EventVersion, Nanoid, OrgRole, OrganizationMembership,
    OrganizationMembershipStatus,
};
use kernel::test_utils::AccountBuilder;
use kernel::KernelError;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct MockConnection;
impl Connection for MockConnection {}

#[derive(Clone)]
struct MockDatabase;
impl DatabaseConnection for MockDatabase {
    type Connection = MockConnection;
    async fn connection(&self) -> error_stack::Result<Self::Connection, KernelError> {
        Ok(MockConnection)
    }
}
impl TransactionManager for MockDatabase {
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
        Box::pin(async move { operation(&mut MockConnection).await })
    }
}

#[derive(Clone)]
struct MockAccounts {
    values: Arc<Mutex<Vec<Account>>>,
    links: Arc<Mutex<Vec<(AccountId, AuthAccountId)>>>,
}
impl AccountQuery for MockAccounts {
    type Connection = MockConnection;
    async fn find_by_id(
        &self,
        _: &mut MockConnection,
        id: &AccountId,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        Ok(self
            .values
            .lock()
            .unwrap()
            .iter()
            .find(|a| a.id() == id && a.deleted_at().is_none())
            .cloned())
    }
    async fn find_by_auth_id(
        &self,
        _: &mut MockConnection,
        _: &AuthAccountId,
    ) -> error_stack::Result<Vec<Account>, KernelError> {
        Ok(self
            .values
            .lock()
            .unwrap()
            .iter()
            .filter(|a| a.kind() == &AccountKind::Personal)
            .cloned()
            .collect())
    }
    async fn find_auth_account_id_by_account_id(
        &self,
        _: &mut MockConnection,
        _: &AccountId,
    ) -> error_stack::Result<Option<AuthAccountId>, KernelError> {
        Ok(None)
    }
    async fn find_by_name(
        &self,
        _: &mut MockConnection,
        name: &AccountName,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        Ok(self
            .values
            .lock()
            .unwrap()
            .iter()
            .find(|a| a.name() == name)
            .cloned())
    }
    async fn find_by_nanoid(
        &self,
        _: &mut MockConnection,
        nanoid: &Nanoid<Account>,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        Ok(self
            .values
            .lock()
            .unwrap()
            .iter()
            .find(|a| a.nanoid() == nanoid && a.deleted_at().is_none())
            .cloned())
    }
    async fn find_by_nanoids(
        &self,
        _: &mut MockConnection,
        nanoids: &[Nanoid<Account>],
    ) -> error_stack::Result<Vec<Account>, KernelError> {
        Ok(self
            .values
            .lock()
            .unwrap()
            .iter()
            .filter(|a| nanoids.contains(a.nanoid()))
            .cloned()
            .collect())
    }
    async fn find_by_id_unfiltered(
        &self,
        _: &mut MockConnection,
        id: &AccountId,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        Ok(self
            .values
            .lock()
            .unwrap()
            .iter()
            .find(|a| a.id() == id)
            .cloned())
    }
    async fn find_by_nanoid_unfiltered(
        &self,
        executor: &mut MockConnection,
        nanoid: &Nanoid<Account>,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        AccountQuery::find_by_nanoid(self, executor, nanoid).await
    }
    async fn find_by_nanoids_unfiltered(
        &self,
        executor: &mut MockConnection,
        nanoids: &[Nanoid<Account>],
    ) -> error_stack::Result<Vec<Account>, KernelError> {
        AccountQuery::find_by_nanoids(self, executor, nanoids).await
    }
    async fn find_by_nanoid_including_deleted(
        &self,
        _: &mut MockConnection,
        nanoid: &Nanoid<Account>,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        Ok(self
            .values
            .lock()
            .unwrap()
            .iter()
            .find(|a| a.nanoid() == nanoid)
            .cloned())
    }
    async fn is_linked_including_deleted(
        &self,
        _: &mut MockConnection,
        _: &AuthAccountId,
        _: &AccountId,
    ) -> error_stack::Result<bool, KernelError> {
        Ok(false)
    }
}

impl AccountReadModel for MockAccounts {
    type Connection = MockConnection;

    async fn find_by_id(
        &self,
        executor: &mut MockConnection,
        id: &AccountId,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        AccountQuery::find_by_id(self, executor, id).await
    }
    async fn find_by_auth_id(
        &self,
        executor: &mut MockConnection,
        auth_id: &AuthAccountId,
    ) -> error_stack::Result<Vec<Account>, KernelError> {
        AccountQuery::find_by_auth_id(self, executor, auth_id).await
    }
    async fn find_auth_account_id_by_account_id(
        &self,
        executor: &mut MockConnection,
        account_id: &AccountId,
    ) -> error_stack::Result<Option<AuthAccountId>, KernelError> {
        AccountQuery::find_auth_account_id_by_account_id(self, executor, account_id).await
    }
    async fn find_by_name(
        &self,
        executor: &mut MockConnection,
        name: &AccountName,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        AccountQuery::find_by_name(self, executor, name).await
    }
    async fn find_by_nanoid(
        &self,
        executor: &mut MockConnection,
        nanoid: &Nanoid<Account>,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        AccountQuery::find_by_nanoid(self, executor, nanoid).await
    }
    async fn find_by_nanoids(
        &self,
        executor: &mut MockConnection,
        nanoids: &[Nanoid<Account>],
    ) -> error_stack::Result<Vec<Account>, KernelError> {
        AccountQuery::find_by_nanoids(self, executor, nanoids).await
    }
    async fn find_warnings(
        &self,
        _: &mut MockConnection,
        _: &AccountId,
    ) -> error_stack::Result<Vec<kernel::interfaces::read_model::AccountWarning>, KernelError> {
        Ok(Vec::new())
    }
    async fn create(
        &self,
        _: &mut MockConnection,
        account: &Account,
    ) -> error_stack::Result<(), KernelError> {
        self.values.lock().unwrap().push(account.clone());
        Ok(())
    }
    async fn update(
        &self,
        _: &mut MockConnection,
        account: &Account,
    ) -> error_stack::Result<(), KernelError> {
        let mut values = self.values.lock().unwrap();
        let value = values
            .iter_mut()
            .find(|value| value.id() == account.id())
            .ok_or_else(|| Report::new(KernelError::NotFound))?;
        *value = account.clone();
        Ok(())
    }
    async fn deactivate(
        &self,
        _: &mut MockConnection,
        _: &AccountId,
    ) -> error_stack::Result<(), KernelError> {
        Ok(())
    }
    async fn unlink_all_auth_accounts(
        &self,
        _: &mut MockConnection,
        account_id: &AccountId,
    ) -> error_stack::Result<(), KernelError> {
        self.links
            .lock()
            .unwrap()
            .retain(|(linked, _)| linked != account_id);
        Ok(())
    }
    async fn link_auth_account(
        &self,
        _: &mut MockConnection,
        account_id: &AccountId,
        auth_id: &AuthAccountId,
    ) -> error_stack::Result<(), KernelError> {
        self.links
            .lock()
            .unwrap()
            .push((account_id.clone(), auth_id.clone()));
        Ok(())
    }
    async fn find_by_id_unfiltered(
        &self,
        executor: &mut MockConnection,
        id: &AccountId,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        AccountQuery::find_by_id_unfiltered(self, executor, id).await
    }
    async fn find_by_nanoid_unfiltered(
        &self,
        executor: &mut MockConnection,
        nanoid: &Nanoid<Account>,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        AccountQuery::find_by_nanoid_unfiltered(self, executor, nanoid).await
    }
    async fn find_by_nanoids_unfiltered(
        &self,
        executor: &mut MockConnection,
        nanoids: &[Nanoid<Account>],
    ) -> error_stack::Result<Vec<Account>, KernelError> {
        AccountQuery::find_by_nanoids_unfiltered(self, executor, nanoids).await
    }
    async fn find_by_id_including_deleted(
        &self,
        executor: &mut MockConnection,
        id: &AccountId,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        AccountQuery::find_by_id_unfiltered(self, executor, id).await
    }
    async fn find_by_nanoid_including_deleted(
        &self,
        executor: &mut MockConnection,
        nanoid: &Nanoid<Account>,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        AccountQuery::find_by_nanoid_including_deleted(self, executor, nanoid).await
    }
    async fn is_linked_including_deleted(
        &self,
        _: &mut MockConnection,
        auth_id: &AuthAccountId,
        account_id: &AccountId,
    ) -> error_stack::Result<bool, KernelError> {
        Ok(self
            .links
            .lock()
            .unwrap()
            .contains(&(account_id.clone(), auth_id.clone())))
    }
    async fn suspend(
        &self,
        _: &mut MockConnection,
        _: &AccountId,
        _: &str,
        _: Option<time::OffsetDateTime>,
    ) -> error_stack::Result<(), KernelError> {
        Ok(())
    }
    async fn unsuspend(
        &self,
        _: &mut MockConnection,
        _: &AccountId,
    ) -> error_stack::Result<(), KernelError> {
        Ok(())
    }
    async fn ban(
        &self,
        _: &mut MockConnection,
        _: &AccountId,
        _: &str,
    ) -> error_stack::Result<(), KernelError> {
        Ok(())
    }
}

#[derive(Clone, Default)]
struct MockAccountRepository {
    saved_events: Arc<Mutex<Vec<AccountEvent>>>,
}

impl AggregateRepository<Account> for MockAccountRepository {
    type Connection = MockConnection;
    type Id = AccountId;

    async fn load(
        &self,
        _: &mut MockConnection,
        _: &AccountId,
    ) -> error_stack::Result<Rehydrated<Account>, KernelError> {
        Err(Report::new(KernelError::NotFound))
    }
    async fn save(
        &self,
        _: &mut MockConnection,
        command: CommandEnvelope<AccountEvent, Account>,
    ) -> error_stack::Result<EventEnvelope<AccountEvent, Account>, KernelError> {
        self.saved_events
            .lock()
            .unwrap()
            .push(command.event().clone());
        Ok(EventEnvelope::new(
            command.id().clone(),
            command.event().clone(),
            EventVersion::default(),
        ))
    }
}

#[derive(Clone)]
struct MockMemberships(Arc<Mutex<Vec<OrganizationMembership>>>);
impl OrganizationMembershipRepository for MockMemberships {
    type Connection = MockConnection;
    async fn create(
        &self,
        _: &mut MockConnection,
        value: &OrganizationMembership,
    ) -> error_stack::Result<(), KernelError> {
        self.0.lock().unwrap().push(value.clone());
        Ok(())
    }
    async fn find(
        &self,
        _: &mut MockConnection,
        org: &AccountId,
        member: &AccountId,
    ) -> error_stack::Result<Option<OrganizationMembership>, KernelError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .iter()
            .find(|m| m.org_account_id() == org && m.member_account_id() == member)
            .cloned())
    }
    async fn find_by_org(
        &self,
        _: &mut MockConnection,
        org: &AccountId,
    ) -> error_stack::Result<Vec<OrganizationMembership>, KernelError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|m| m.org_account_id() == org)
            .cloned()
            .collect())
    }
    async fn find_by_member(
        &self,
        _: &mut MockConnection,
        member: &AccountId,
    ) -> error_stack::Result<Vec<OrganizationMembership>, KernelError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|m| m.member_account_id() == member)
            .cloned()
            .collect())
    }
    async fn update_role(
        &self,
        _: &mut MockConnection,
        org: &AccountId,
        member: &AccountId,
        role: OrgRole,
    ) -> error_stack::Result<(), KernelError> {
        let mut values = self.0.lock().unwrap();
        let value = values
            .iter_mut()
            .find(|m| m.org_account_id() == org && m.member_account_id() == member)
            .ok_or_else(|| Report::new(KernelError::NotFound))?;
        *value = OrganizationMembership::new(
            org.clone(),
            member.clone(),
            role,
            *value.status(),
            value.invited_by().clone(),
            value.created_at().clone(),
        );
        Ok(())
    }
    async fn update_status(
        &self,
        _: &mut MockConnection,
        org: &AccountId,
        member: &AccountId,
        status: OrganizationMembershipStatus,
    ) -> error_stack::Result<(), KernelError> {
        let mut values = self.0.lock().unwrap();
        let value = values
            .iter_mut()
            .find(|m| m.org_account_id() == org && m.member_account_id() == member)
            .ok_or_else(|| Report::new(KernelError::NotFound))?;
        *value = OrganizationMembership::new(
            org.clone(),
            member.clone(),
            *value.role(),
            status,
            value.invited_by().clone(),
            value.created_at().clone(),
        );
        Ok(())
    }
    async fn delete(
        &self,
        _: &mut MockConnection,
        org: &AccountId,
        member: &AccountId,
    ) -> error_stack::Result<(), KernelError> {
        let mut values = self.0.lock().unwrap();
        let len = values.len();
        values.retain(|m| m.org_account_id() != org || m.member_account_id() != member);
        if values.len() == len {
            Err(Report::new(KernelError::NotFound))
        } else {
            Ok(())
        }
    }
    async fn count_active_owners(
        &self,
        _: &mut MockConnection,
        org: &AccountId,
    ) -> error_stack::Result<i64, KernelError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|m| {
                m.org_account_id() == org
                    && m.role() == &OrgRole::Owner
                    && m.status() == &OrganizationMembershipStatus::Active
            })
            .count() as i64)
    }
    async fn lock_active_owner_rows(
        &self,
        _: &mut MockConnection,
        _: &AccountId,
    ) -> error_stack::Result<(), KernelError> {
        Ok(())
    }
}

#[derive(Clone)]
struct Module {
    database: MockDatabase,
    accounts: MockAccounts,
    account_repository: MockAccountRepository,
    memberships: MockMemberships,
}
impl DependOnDatabaseConnection for Module {
    type DatabaseConnection = MockDatabase;
    fn database_connection(&self) -> &MockDatabase {
        &self.database
    }
}
impl DependOnTransactionManager for Module {
    type TransactionManager = MockDatabase;
    fn transaction_manager(&self) -> &MockDatabase {
        &self.database
    }
}
impl DependOnAccountReadModel for Module {
    type AccountReadModel = MockAccounts;
    fn account_read_model(&self) -> &MockAccounts {
        &self.accounts
    }
}
impl DependOnAccountRepository for Module {
    type AccountRepository = MockAccountRepository;
    fn account_repository(&self) -> &MockAccountRepository {
        &self.account_repository
    }
}
impl DependOnOrganizationMembershipRepository for Module {
    type OrganizationMembershipRepository = MockMemberships;
    fn organization_membership_repository(&self) -> &MockMemberships {
        &self.memberships
    }
}

struct Fixture {
    module: Module,
    auth: AuthAccountId,
    member: Account,
}
fn fixture(owner_count: usize) -> Fixture {
    kernel::ensure_generator_initialized();
    let org = AccountBuilder::new()
        .kind(AccountKind::Organization)
        .nanoid(Nanoid::new("org"))
        .build();
    let owner = AccountBuilder::new().nanoid(Nanoid::new("owner")).build();
    let member = AccountBuilder::new().nanoid(Nanoid::new("member")).build();
    let mut memberships = vec![OrganizationMembership::new(
        org.id().clone(),
        owner.id().clone(),
        OrgRole::Owner,
        OrganizationMembershipStatus::Active,
        owner.id().clone(),
        CreatedAt::now(),
    )];
    if owner_count > 1 {
        memberships.push(OrganizationMembership::new(
            org.id().clone(),
            member.id().clone(),
            OrgRole::Owner,
            OrganizationMembershipStatus::Active,
            owner.id().clone(),
            CreatedAt::now(),
        ));
    }
    Fixture {
        module: Module {
            database: MockDatabase,
            accounts: MockAccounts {
                values: Arc::new(Mutex::new(vec![org.clone(), owner.clone(), member.clone()])),
                links: Arc::new(Mutex::new(Vec::new())),
            },
            account_repository: MockAccountRepository::default(),
            memberships: MockMemberships(Arc::new(Mutex::new(memberships))),
        },
        auth: AuthAccountId::default(),
        member,
    }
}

#[tokio::test]
async fn create_organization_persists_organization_owner_and_auth_link() {
    let f = fixture(1);
    let creator = f
        .module
        .accounts
        .values
        .lock()
        .unwrap()
        .iter()
        .find(|account| account.nanoid().as_ref() == "owner")
        .unwrap()
        .clone();

    let result = f
        .module
        .create_organization(
            f.auth.clone(),
            crate::dto::organization::CreateOrganizationDto {
                name: "created-org".to_string(),
            },
        )
        .await
        .unwrap();

    let accounts = f.module.accounts.values.lock().unwrap();
    let organization = accounts
        .iter()
        .find(|account| account.nanoid().as_ref() == &result.account_id)
        .unwrap();
    assert_eq!(organization.kind(), &AccountKind::Organization);
    let memberships = f.module.memberships.0.lock().unwrap();
    assert!(memberships.iter().any(|membership| {
        membership.org_account_id() == organization.id()
            && membership.member_account_id() == creator.id()
            && membership.role() == &OrgRole::Owner
            && membership.status() == &OrganizationMembershipStatus::Active
    }));
    assert!(f
        .module
        .accounts
        .links
        .lock()
        .unwrap()
        .contains(&(organization.id().clone(), f.auth.clone())));
    assert!(f
        .module
        .account_repository
        .saved_events
        .lock()
        .unwrap()
        .iter()
        .any(|event| matches!(event, AccountEvent::Created { kind: AccountKind::Organization, auth_account_id, .. } if auth_account_id == &f.auth)));
}

#[tokio::test]
async fn invite_accept_flow_creates_pending_then_active_membership() {
    let f = fixture(1);
    f.module
        .invite_member(
            f.auth.clone(),
            "org".into(),
            "member".into(),
            OrgRole::Member,
        )
        .await
        .unwrap();
    f.module
        .accept_invite(f.auth, "org".into(), "member".into())
        .await
        .unwrap();
    let values = f.module.memberships.0.lock().unwrap();
    assert!(values.iter().any(|m| m.member_account_id() == f.member.id()
        && m.status() == &OrganizationMembershipStatus::Active));
}

#[tokio::test]
async fn duplicate_invite_is_rejected() {
    let f = fixture(1);
    f.module
        .invite_member(
            f.auth.clone(),
            "org".into(),
            "member".into(),
            OrgRole::Member,
        )
        .await
        .unwrap();
    let error = f
        .module
        .invite_member(f.auth, "org".into(), "member".into(), OrgRole::Member)
        .await
        .unwrap_err();
    assert_eq!(error.current_context(), &KernelError::Rejected);
}

#[tokio::test]
async fn owner_invite_is_rejected() {
    let f = fixture(1);

    let error = f
        .module
        .invite_member(f.auth, "org".into(), "member".into(), OrgRole::Owner)
        .await
        .unwrap_err();

    assert_eq!(error.current_context(), &KernelError::Rejected);
}

#[tokio::test]
async fn non_member_management_is_permission_denied() {
    let f = fixture(1);
    f.module.memberships.0.lock().unwrap().clear();

    let error = f
        .module
        .invite_member(f.auth, "org".into(), "member".into(), OrgRole::Member)
        .await
        .unwrap_err();

    assert_eq!(error.current_context(), &KernelError::PermissionDenied);
}

#[tokio::test]
async fn nonexistent_organization_is_not_found() {
    let f = fixture(1);

    let error = f
        .module
        .invite_member(f.auth, "missing".into(), "member".into(), OrgRole::Member)
        .await
        .unwrap_err();

    assert_eq!(error.current_context(), &KernelError::NotFound);
}

#[tokio::test]
async fn last_owner_cannot_be_demoted_or_leave() {
    let f = fixture(1);
    let demote = f
        .module
        .change_role(
            f.auth.clone(),
            "org".into(),
            "owner".into(),
            OrgRole::Member,
        )
        .await
        .unwrap_err();
    let leave = f
        .module
        .leave_organization(f.auth, "org".into(), "owner".into())
        .await
        .unwrap_err();
    assert_eq!(demote.current_context(), &KernelError::Rejected);
    assert_eq!(leave.current_context(), &KernelError::Rejected);
}

#[tokio::test]
async fn owner_changes_active_member_role_to_admin() {
    let f = fixture(1);
    let owner = f
        .module
        .accounts
        .values
        .lock()
        .unwrap()
        .iter()
        .find(|account| account.nanoid().as_ref() == "owner")
        .unwrap()
        .clone();
    f.module
        .memberships
        .0
        .lock()
        .unwrap()
        .push(OrganizationMembership::new(
            f.module
                .accounts
                .values
                .lock()
                .unwrap()
                .iter()
                .find(|account| account.nanoid().as_ref() == "org")
                .unwrap()
                .id()
                .clone(),
            f.member.id().clone(),
            OrgRole::Member,
            OrganizationMembershipStatus::Active,
            owner.id().clone(),
            CreatedAt::now(),
        ));

    f.module
        .change_role(f.auth, "org".into(), "member".into(), OrgRole::Admin)
        .await
        .unwrap();

    assert!(f
        .module
        .memberships
        .0
        .lock()
        .unwrap()
        .iter()
        .any(|membership| {
            membership.member_account_id() == f.member.id() && membership.role() == &OrgRole::Admin
        }));
}

#[tokio::test]
async fn owner_removes_active_member() {
    let f = fixture(1);
    let (org, owner) = {
        let accounts = f.module.accounts.values.lock().unwrap();
        let org = accounts
            .iter()
            .find(|account| account.nanoid().as_ref() == "org")
            .unwrap()
            .clone();
        let owner = accounts
            .iter()
            .find(|account| account.nanoid().as_ref() == "owner")
            .unwrap()
            .clone();
        (org, owner)
    };
    f.module
        .memberships
        .0
        .lock()
        .unwrap()
        .push(OrganizationMembership::new(
            org.id().clone(),
            f.member.id().clone(),
            OrgRole::Member,
            OrganizationMembershipStatus::Active,
            owner.id().clone(),
            CreatedAt::now(),
        ));

    f.module
        .remove_member(f.auth, "org".into(), "member".into())
        .await
        .unwrap();

    assert!(!f
        .module
        .memberships
        .0
        .lock()
        .unwrap()
        .iter()
        .any(|membership| {
            membership.org_account_id() == org.id()
                && membership.member_account_id() == f.member.id()
        }));
}

#[tokio::test]
async fn owner_cannot_be_removed_but_member_can_leave() {
    let f = fixture(2);
    let remove = f
        .module
        .remove_member(f.auth.clone(), "org".into(), "owner".into())
        .await
        .unwrap_err();
    assert_eq!(remove.current_context(), &KernelError::Rejected);
    f.module
        .leave_organization(f.auth, "org".into(), "member".into())
        .await
        .unwrap();
}
