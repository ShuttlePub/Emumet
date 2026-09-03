use super::database::MockConnection;
use error_stack::Report;
use kernel::interfaces::read_model::{
    ProfileTransferRequestProjection, ProfileTransferRequestReadModel,
};
use kernel::interfaces::repository::{AggregateRepository, Rehydrated};
use kernel::interfaces::storage::{ProfileMediaCopyGateway, ProfileMediaCopyRequest};
use kernel::prelude::entity::{
    AccountId, CommandEnvelope, EventEnvelope, EventVersion, Nanoid, ProfileId,
    ProfileTransferRequest, ProfileTransferRequestEvent, ProfileTransferRequestId,
    ProfileTransferStatus,
};
use kernel::KernelError;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct MockProfileTransferRequestRepository {
    pub request: Option<ProfileTransferRequest>,
    pub saved_events: Arc<Mutex<Vec<ProfileTransferRequestEvent>>>,
}

impl AggregateRepository<ProfileTransferRequest> for MockProfileTransferRequestRepository {
    type Connection = MockConnection;
    type Id = ProfileTransferRequestId;

    async fn load(
        &self,
        _executor: &mut Self::Connection,
        id: &Self::Id,
    ) -> error_stack::Result<Rehydrated<ProfileTransferRequest>, KernelError> {
        self.request
            .as_ref()
            .filter(|request| request.id() == id)
            .map(|request| Rehydrated::new(request.clone(), request.version().clone()))
            .ok_or_else(|| Report::new(KernelError::NotFound))
    }

    async fn save(
        &self,
        _executor: &mut Self::Connection,
        command: CommandEnvelope<ProfileTransferRequestEvent, ProfileTransferRequest>,
    ) -> error_stack::Result<
        EventEnvelope<ProfileTransferRequestEvent, ProfileTransferRequest>,
        KernelError,
    > {
        self.saved_events
            .lock()
            .unwrap()
            .push(command.event().clone());
        Ok(EventEnvelope::new(
            command.id().clone(),
            command.event().clone(),
            EventVersion::new(command.id().as_ref() + 1),
        ))
    }
}

#[derive(Clone)]
pub struct MockProfileTransferRequestReadModel {
    pub requests: Arc<Mutex<Vec<ProfileTransferRequestProjection>>>,
    pub created: Arc<Mutex<Vec<ProfileTransferRequestProjection>>>,
    pub updated: Arc<Mutex<Vec<ProfileTransferRequestProjection>>>,
}

impl ProfileTransferRequestReadModel for MockProfileTransferRequestReadModel {
    type Connection = MockConnection;

    async fn find_by_id(
        &self,
        _executor: &mut Self::Connection,
        id: &ProfileTransferRequestId,
    ) -> error_stack::Result<Option<ProfileTransferRequestProjection>, KernelError> {
        Ok(self
            .requests
            .lock()
            .unwrap()
            .iter()
            .find(|request| request.id() == id)
            .cloned())
    }

    async fn find_by_id_unfiltered(
        &self,
        executor: &mut Self::Connection,
        id: &ProfileTransferRequestId,
    ) -> error_stack::Result<Option<ProfileTransferRequestProjection>, KernelError> {
        self.find_by_id(executor, id).await
    }

    async fn find_by_nanoid(
        &self,
        _executor: &mut Self::Connection,
        nanoid: &Nanoid<ProfileTransferRequest>,
    ) -> error_stack::Result<Option<ProfileTransferRequestProjection>, KernelError> {
        Ok(self
            .requests
            .lock()
            .unwrap()
            .iter()
            .find(|request| request.nanoid() == nanoid)
            .cloned())
    }

    async fn find_pending_by_profile_id(
        &self,
        _executor: &mut Self::Connection,
        profile_id: &ProfileId,
    ) -> error_stack::Result<Option<ProfileTransferRequestProjection>, KernelError> {
        Ok(self
            .requests
            .lock()
            .unwrap()
            .iter()
            .find(|request| {
                request.profile_id() == profile_id
                    && request.status() == &ProfileTransferStatus::Pending
            })
            .cloned())
    }

    async fn create(
        &self,
        _executor: &mut Self::Connection,
        request: &ProfileTransferRequest,
    ) -> error_stack::Result<(), KernelError> {
        let projection = request.clone().into();
        self.created.lock().unwrap().push(projection);
        Ok(())
    }

    async fn update(
        &self,
        _executor: &mut Self::Connection,
        request: &ProfileTransferRequest,
    ) -> error_stack::Result<(), KernelError> {
        let projection: ProfileTransferRequestProjection = request.clone().into();
        let mut requests = self.requests.lock().unwrap();
        if let Some(existing) = requests
            .iter_mut()
            .find(|item| item.id() == projection.id())
        {
            *existing = projection.clone();
        }
        self.updated.lock().unwrap().push(projection);
        Ok(())
    }
}

#[derive(Clone)]
pub struct MockProfileMediaCopyGateway {
    pub recorded: Arc<Mutex<Vec<ProfileMediaCopyRequest>>>,
}

impl ProfileMediaCopyGateway for MockProfileMediaCopyGateway {
    async fn request_copy(
        &self,
        request: ProfileMediaCopyRequest,
    ) -> error_stack::Result<(), KernelError> {
        self.recorded.lock().unwrap().push(request);
        Ok(())
    }
}

pub fn active_membership(
    org_id: AccountId,
    member_id: AccountId,
    role: kernel::prelude::entity::OrgRole,
) -> kernel::prelude::entity::OrganizationMembership {
    kernel::prelude::entity::OrganizationMembership::new(
        org_id,
        member_id.clone(),
        role,
        kernel::prelude::entity::OrganizationMembershipStatus::Active,
        member_id,
        kernel::prelude::entity::CreatedAt::now(),
    )
}
