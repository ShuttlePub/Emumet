mod query;

pub use self::query::*;

use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::{
    AccountId, EventVersion, Nanoid, ProfileId, ProfileTransferRequest, ProfileTransferRequestId,
    ProfileTransferStatus,
};
use crate::KernelError;
use std::future::Future;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProfileTransferRequestProjection {
    id: ProfileTransferRequestId,
    profile_id: ProfileId,
    from_account_id: AccountId,
    to_org_account_id: AccountId,
    status: ProfileTransferStatus,
    version: EventVersion<ProfileTransferRequest>,
    nanoid: Nanoid<ProfileTransferRequest>,
}

impl ProfileTransferRequestProjection {
    pub fn new(
        id: ProfileTransferRequestId,
        profile_id: ProfileId,
        from_account_id: AccountId,
        to_org_account_id: AccountId,
        status: ProfileTransferStatus,
        version: EventVersion<ProfileTransferRequest>,
        nanoid: Nanoid<ProfileTransferRequest>,
    ) -> Self {
        Self {
            id,
            profile_id,
            from_account_id,
            to_org_account_id,
            status,
            version,
            nanoid,
        }
    }

    pub fn id(&self) -> &ProfileTransferRequestId {
        &self.id
    }

    pub fn profile_id(&self) -> &ProfileId {
        &self.profile_id
    }

    pub fn from_account_id(&self) -> &AccountId {
        &self.from_account_id
    }

    pub fn to_org_account_id(&self) -> &AccountId {
        &self.to_org_account_id
    }

    pub fn status(&self) -> &ProfileTransferStatus {
        &self.status
    }

    pub fn version(&self) -> &EventVersion<ProfileTransferRequest> {
        &self.version
    }

    pub fn nanoid(&self) -> &Nanoid<ProfileTransferRequest> {
        &self.nanoid
    }
}

impl From<ProfileTransferRequest> for ProfileTransferRequestProjection {
    fn from(value: ProfileTransferRequest) -> Self {
        let destruct = value.into_destruct();
        Self::new(
            destruct.id,
            destruct.profile_id,
            destruct.from_account_id,
            destruct.to_org_account_id,
            destruct.status,
            destruct.version,
            destruct.nanoid,
        )
    }
}

impl From<ProfileTransferRequestProjection> for ProfileTransferRequest {
    fn from(value: ProfileTransferRequestProjection) -> Self {
        ProfileTransferRequest::reconstitute(
            value.id().clone(),
            value.profile_id().clone(),
            value.from_account_id().clone(),
            value.to_org_account_id().clone(),
            value.status().clone(),
            value.version().clone(),
            value.nanoid().clone(),
        )
    }
}

pub trait ProfileTransferRequestReadModel: Sync + Send + 'static {
    type Connection: Connection;

    fn find_by_id(
        &self,
        executor: &mut Self::Connection,
        id: &ProfileTransferRequestId,
    ) -> impl Future<
        Output = error_stack::Result<Option<ProfileTransferRequestProjection>, KernelError>,
    > + Send;

    fn find_by_id_unfiltered(
        &self,
        executor: &mut Self::Connection,
        id: &ProfileTransferRequestId,
    ) -> impl Future<
        Output = error_stack::Result<Option<ProfileTransferRequestProjection>, KernelError>,
    > + Send;

    fn find_by_nanoid(
        &self,
        executor: &mut Self::Connection,
        nanoid: &Nanoid<ProfileTransferRequest>,
    ) -> impl Future<
        Output = error_stack::Result<Option<ProfileTransferRequestProjection>, KernelError>,
    > + Send;

    fn find_pending_by_profile_id(
        &self,
        executor: &mut Self::Connection,
        profile_id: &ProfileId,
    ) -> impl Future<
        Output = error_stack::Result<Option<ProfileTransferRequestProjection>, KernelError>,
    > + Send;

    fn create(
        &self,
        executor: &mut Self::Connection,
        request: &ProfileTransferRequest,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn update(
        &self,
        executor: &mut Self::Connection,
        request: &ProfileTransferRequest,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnProfileTransferRequestReadModel:
    Sync + Send + DependOnDatabaseConnection
{
    type ProfileTransferRequestReadModel: ProfileTransferRequestReadModel<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn profile_transfer_request_read_model(&self) -> &Self::ProfileTransferRequestReadModel;
}
