use super::account::MockAccountQuery;
use super::database::MockDatabaseConnection;
use super::profile::{MockProfileReadModel, MockProfileRepository};
use super::request::{
    active_membership, MockProfileMediaCopyGateway, MockProfileTransferRequestReadModel,
    MockProfileTransferRequestRepository,
};
use kernel::interfaces::database::{DependOnDatabaseConnection, DependOnTransactionManager};
use kernel::interfaces::read_model::ProfileTransferRequestProjection;
use kernel::interfaces::read_model::{
    DependOnAccountReadModel, DependOnProfileReadModel, DependOnProfileTransferRequestReadModel,
};
use kernel::interfaces::repository::{
    DependOnOrganizationMembershipRepository, DependOnProfileRepository,
    DependOnProfileTransferRequestRepository, OrganizationMembershipRepository,
};
use kernel::interfaces::storage::DependOnProfileMediaCopyGateway;
use kernel::prelude::entity::{
    Account, AccountId, AccountKind, AuthAccountId, EventVersion, Nanoid, OrgRole,
    OrganizationMembership, OrganizationMembershipStatus, Profile, ProfileId,
    ProfileTransferRequest, ProfileTransferRequestId, ProfileTransferStatus,
};
use kernel::test_utils::{AccountBuilder, ProfileBuilder};
use std::sync::{Arc, Mutex};

use super::database::MockConnection;
use error_stack::Report;
use kernel::KernelError;

#[derive(Clone)]
pub struct MockModule {
    pub database: MockDatabaseConnection,
    pub accounts: MockAccountQuery,
    pub profiles: MockProfileReadModel,
    pub profile_repository: MockProfileRepository,
    pub transfer_requests: MockProfileTransferRequestReadModel,
    pub transfer_request_repository: MockProfileTransferRequestRepository,
    pub memberships: Arc<Mutex<Vec<OrganizationMembership>>>,
    pub media_copy_gateway: MockProfileMediaCopyGateway,
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

impl DependOnAccountReadModel for MockModule {
    type AccountReadModel = MockAccountQuery;

    fn account_read_model(&self) -> &Self::AccountReadModel {
        &self.accounts
    }
}

impl DependOnProfileReadModel for MockModule {
    type ProfileReadModel = MockProfileReadModel;

    fn profile_read_model(&self) -> &Self::ProfileReadModel {
        &self.profiles
    }
}

impl DependOnProfileRepository for MockModule {
    type ProfileRepository = MockProfileRepository;

    fn profile_repository(&self) -> &Self::ProfileRepository {
        &self.profile_repository
    }
}

impl DependOnProfileTransferRequestReadModel for MockModule {
    type ProfileTransferRequestReadModel = MockProfileTransferRequestReadModel;

    fn profile_transfer_request_read_model(&self) -> &Self::ProfileTransferRequestReadModel {
        &self.transfer_requests
    }
}

impl DependOnProfileTransferRequestRepository for MockModule {
    type ProfileTransferRequestRepository = MockProfileTransferRequestRepository;

    fn profile_transfer_request_repository(&self) -> &Self::ProfileTransferRequestRepository {
        &self.transfer_request_repository
    }
}

impl OrganizationMembershipRepository for MockModule {
    type Connection = MockConnection;

    async fn create(
        &self,
        _executor: &mut Self::Connection,
        membership: &OrganizationMembership,
    ) -> error_stack::Result<(), KernelError> {
        self.memberships.lock().unwrap().push(membership.clone());
        Ok(())
    }

    async fn find(
        &self,
        _executor: &mut Self::Connection,
        org_account_id: &AccountId,
        member_account_id: &AccountId,
    ) -> error_stack::Result<Option<OrganizationMembership>, KernelError> {
        Ok(self
            .memberships
            .lock()
            .unwrap()
            .iter()
            .find(|m| {
                m.org_account_id() == org_account_id && m.member_account_id() == member_account_id
            })
            .cloned())
    }

    async fn find_by_org(
        &self,
        _executor: &mut Self::Connection,
        org_account_id: &AccountId,
    ) -> error_stack::Result<Vec<OrganizationMembership>, KernelError> {
        Ok(self
            .memberships
            .lock()
            .unwrap()
            .iter()
            .filter(|m| m.org_account_id() == org_account_id)
            .cloned()
            .collect())
    }

    async fn find_by_member(
        &self,
        _executor: &mut Self::Connection,
        member_account_id: &AccountId,
    ) -> error_stack::Result<Vec<OrganizationMembership>, KernelError> {
        Ok(self
            .memberships
            .lock()
            .unwrap()
            .iter()
            .filter(|m| m.member_account_id() == member_account_id)
            .cloned()
            .collect())
    }

    async fn update_role(
        &self,
        _executor: &mut Self::Connection,
        org_account_id: &AccountId,
        member_account_id: &AccountId,
        role: OrgRole,
    ) -> error_stack::Result<(), KernelError> {
        let mut values = self.memberships.lock().unwrap();
        let value = values
            .iter_mut()
            .find(|m| {
                m.org_account_id() == org_account_id && m.member_account_id() == member_account_id
            })
            .ok_or_else(|| Report::new(KernelError::NotFound))?;
        *value = OrganizationMembership::new(
            org_account_id.clone(),
            member_account_id.clone(),
            role,
            *value.status(),
            value.invited_by().clone(),
            value.created_at().clone(),
        );
        Ok(())
    }

    async fn update_status(
        &self,
        _executor: &mut Self::Connection,
        org_account_id: &AccountId,
        member_account_id: &AccountId,
        status: OrganizationMembershipStatus,
    ) -> error_stack::Result<(), KernelError> {
        let mut values = self.memberships.lock().unwrap();
        let value = values
            .iter_mut()
            .find(|m| {
                m.org_account_id() == org_account_id && m.member_account_id() == member_account_id
            })
            .ok_or_else(|| Report::new(KernelError::NotFound))?;
        *value = OrganizationMembership::new(
            org_account_id.clone(),
            member_account_id.clone(),
            *value.role(),
            status,
            value.invited_by().clone(),
            value.created_at().clone(),
        );
        Ok(())
    }

    async fn delete(
        &self,
        _executor: &mut Self::Connection,
        org_account_id: &AccountId,
        member_account_id: &AccountId,
    ) -> error_stack::Result<(), KernelError> {
        let mut values = self.memberships.lock().unwrap();
        let len = values.len();
        values.retain(|m| {
            m.org_account_id() != org_account_id || m.member_account_id() != member_account_id
        });
        if values.len() == len {
            Err(Report::new(KernelError::NotFound))
        } else {
            Ok(())
        }
    }

    async fn count_active_owners(
        &self,
        _executor: &mut Self::Connection,
        org_account_id: &AccountId,
    ) -> error_stack::Result<i64, KernelError> {
        Ok(self
            .memberships
            .lock()
            .unwrap()
            .iter()
            .filter(|m| {
                m.org_account_id() == org_account_id
                    && m.role() == &OrgRole::Owner
                    && m.status() == &OrganizationMembershipStatus::Active
            })
            .count() as i64)
    }

    async fn lock_active_owner_rows(
        &self,
        _executor: &mut Self::Connection,
        _org_account_id: &AccountId,
    ) -> error_stack::Result<(), KernelError> {
        Ok(())
    }
}

impl DependOnOrganizationMembershipRepository for MockModule {
    type OrganizationMembershipRepository = Self;

    fn organization_membership_repository(&self) -> &Self::OrganizationMembershipRepository {
        self
    }
}

impl DependOnProfileMediaCopyGateway for MockModule {
    type ProfileMediaCopyGateway = MockProfileMediaCopyGateway;

    fn profile_media_copy_gateway(&self) -> &Self::ProfileMediaCopyGateway {
        &self.media_copy_gateway
    }
}

pub struct Fixture {
    pub module: MockModule,
    pub auth: AuthAccountId,
    pub owner_account: Account,
    pub org_account: Account,
    pub profile: Profile,
    pub profile_nanoid: String,
    pub request_nanoid: String,
}

pub fn fixture() -> Fixture {
    kernel::ensure_generator_initialized();
    let auth = AuthAccountId::default();

    let owner_account = AccountBuilder::new()
        .id(AccountId::new(100))
        .nanoid(Nanoid::new("owner"))
        .kind(AccountKind::Personal)
        .build();
    let org_account = AccountBuilder::new()
        .id(AccountId::new(200))
        .nanoid(Nanoid::new("org"))
        .kind(AccountKind::Organization)
        .build();

    let profile_id = ProfileId::new(1);
    let profile_nanoid = Nanoid::<Profile>::new("profile-1");
    let profile = ProfileBuilder::new()
        .id(profile_id.clone())
        .account_id(owner_account.id().clone())
        .nanoid(profile_nanoid.clone())
        .build();

    let request_id = ProfileTransferRequestId::new(10);
    let request_nanoid = Nanoid::<ProfileTransferRequest>::new("request-1");
    let request = ProfileTransferRequest::new(
        request_id.clone(),
        profile_id.clone(),
        owner_account.id().clone(),
        org_account.id().clone(),
        ProfileTransferStatus::Pending,
        EventVersion::default(),
        request_nanoid.clone(),
    );

    let request_projection: ProfileTransferRequestProjection = request.clone().into();

    let memberships = vec![active_membership(
        org_account.id().clone(),
        owner_account.id().clone(),
        OrgRole::Owner,
    )];

    let module = MockModule {
        database: MockDatabaseConnection,
        accounts: MockAccountQuery {
            owned_accounts: vec![owner_account.clone()],
            target: Some(org_account.clone()),
        },
        profiles: MockProfileReadModel {
            profiles: Arc::new(Mutex::new(vec![profile.clone().into()])),
            updated: Arc::new(Mutex::new(Vec::new())),
        },
        profile_repository: MockProfileRepository {
            profile: Some(profile.clone()),
            saved_events: Arc::new(Mutex::new(Vec::new())),
        },
        transfer_requests: MockProfileTransferRequestReadModel {
            requests: Arc::new(Mutex::new(vec![request_projection])),
            created: Arc::new(Mutex::new(Vec::new())),
            updated: Arc::new(Mutex::new(Vec::new())),
        },
        transfer_request_repository: MockProfileTransferRequestRepository {
            request: Some(request),
            saved_events: Arc::new(Mutex::new(Vec::new())),
        },
        memberships: Arc::new(Mutex::new(memberships)),
        media_copy_gateway: MockProfileMediaCopyGateway {
            recorded: Arc::new(Mutex::new(Vec::new())),
        },
    };

    Fixture {
        module,
        auth,
        owner_account,
        org_account,
        profile,
        profile_nanoid: profile_nanoid.as_ref().to_string(),
        request_nanoid: request_nanoid.as_ref().to_string(),
    }
}
