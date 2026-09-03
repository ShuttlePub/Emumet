use super::{
    DependOnProfileTransferRequestReadModel, ProfileTransferRequestProjection,
    ProfileTransferRequestReadModel,
};
use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::{Nanoid, ProfileId, ProfileTransferRequest, ProfileTransferRequestId};
use crate::KernelError;
use std::future::Future;

pub trait ProfileTransferRequestQuery: Send + Sync + 'static {
    type Connection: Connection;

    fn find_by_id(
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
}

impl<T> ProfileTransferRequestQuery for T
where
    T: DependOnProfileTransferRequestReadModel + Send + Sync + 'static,
{
    type Connection = <<T as DependOnProfileTransferRequestReadModel>::ProfileTransferRequestReadModel as ProfileTransferRequestReadModel>::Connection;

    async fn find_by_id(
        &self,
        executor: &mut Self::Connection,
        id: &ProfileTransferRequestId,
    ) -> error_stack::Result<Option<ProfileTransferRequestProjection>, KernelError> {
        self.profile_transfer_request_read_model()
            .find_by_id(executor, id)
            .await
    }

    async fn find_by_nanoid(
        &self,
        executor: &mut Self::Connection,
        nanoid: &Nanoid<ProfileTransferRequest>,
    ) -> error_stack::Result<Option<ProfileTransferRequestProjection>, KernelError> {
        self.profile_transfer_request_read_model()
            .find_by_nanoid(executor, nanoid)
            .await
    }

    async fn find_pending_by_profile_id(
        &self,
        executor: &mut Self::Connection,
        profile_id: &ProfileId,
    ) -> error_stack::Result<Option<ProfileTransferRequestProjection>, KernelError> {
        self.profile_transfer_request_read_model()
            .find_pending_by_profile_id(executor, profile_id)
            .await
    }
}

pub trait DependOnProfileTransferRequestQuery: DependOnDatabaseConnection + Send + Sync {
    type ProfileTransferRequestQuery: ProfileTransferRequestQuery<
        Connection = <<Self as DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Connection,
    >;
    fn profile_transfer_request_query(&self) -> &Self::ProfileTransferRequestQuery;
}

impl<T> DependOnProfileTransferRequestQuery for T
where
    T: DependOnProfileTransferRequestReadModel + DependOnDatabaseConnection + Send + Sync + 'static,
{
    type ProfileTransferRequestQuery = Self;
    fn profile_transfer_request_query(&self) -> &Self::ProfileTransferRequestQuery {
        self
    }
}
