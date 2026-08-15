use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::{
    AccountId, EventVersion, ImageId, Nanoid, Profile, ProfileDisplayName, ProfileId,
    ProfileSummary,
};
use crate::KernelError;
use std::future::Future;

/// Projection DTO for profile reads (ADR 0006 decision 9).
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProfileProjection {
    id: ProfileId,
    account_id: AccountId,
    display_name: Option<ProfileDisplayName>,
    summary: Option<ProfileSummary>,
    icon: Option<ImageId>,
    banner: Option<ImageId>,
    version: EventVersion<Profile>,
    nanoid: Nanoid<Profile>,
}

impl ProfileProjection {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: ProfileId,
        account_id: AccountId,
        display_name: Option<ProfileDisplayName>,
        summary: Option<ProfileSummary>,
        icon: Option<ImageId>,
        banner: Option<ImageId>,
        version: EventVersion<Profile>,
        nanoid: Nanoid<Profile>,
    ) -> Self {
        Self {
            id,
            account_id,
            display_name,
            summary,
            icon,
            banner,
            version,
            nanoid,
        }
    }

    pub fn id(&self) -> &ProfileId {
        &self.id
    }

    pub fn account_id(&self) -> &AccountId {
        &self.account_id
    }

    pub fn display_name(&self) -> &Option<ProfileDisplayName> {
        &self.display_name
    }

    pub fn summary(&self) -> &Option<ProfileSummary> {
        &self.summary
    }

    pub fn icon(&self) -> &Option<ImageId> {
        &self.icon
    }

    pub fn banner(&self) -> &Option<ImageId> {
        &self.banner
    }

    pub fn version(&self) -> &EventVersion<Profile> {
        &self.version
    }

    pub fn nanoid(&self) -> &Nanoid<Profile> {
        &self.nanoid
    }
}

impl From<Profile> for ProfileProjection {
    fn from(value: Profile) -> Self {
        let destruct = value.into_destruct();
        Self::new(
            destruct.id,
            destruct.account_id,
            destruct.display_name,
            destruct.summary,
            destruct.icon,
            destruct.banner,
            destruct.version,
            destruct.nanoid,
        )
    }
}

impl From<ProfileProjection> for Profile {
    fn from(value: ProfileProjection) -> Self {
        Profile::reconstitute(
            value.id().clone(),
            value.account_id().clone(),
            value.display_name().clone(),
            value.summary().clone(),
            value.icon().clone(),
            value.banner().clone(),
            value.version().clone(),
            value.nanoid().clone(),
        )
    }
}

pub trait ProfileReadModel: Sync + Send + 'static {
    type Connection: Connection;

    fn find_by_id(
        &self,
        executor: &mut Self::Connection,
        id: &ProfileId,
    ) -> impl Future<Output = error_stack::Result<Option<ProfileProjection>, KernelError>> + Send;

    fn find_by_id_unfiltered(
        &self,
        executor: &mut Self::Connection,
        id: &ProfileId,
    ) -> impl Future<Output = error_stack::Result<Option<ProfileProjection>, KernelError>> + Send;

    fn find_by_account_id(
        &self,
        executor: &mut Self::Connection,
        account_id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<Option<ProfileProjection>, KernelError>> + Send;

    fn find_by_account_ids(
        &self,
        executor: &mut Self::Connection,
        account_ids: &[AccountId],
    ) -> impl Future<Output = error_stack::Result<Vec<ProfileProjection>, KernelError>> + Send;

    fn create(
        &self,
        executor: &mut Self::Connection,
        profile: &Profile,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn update(
        &self,
        executor: &mut Self::Connection,
        profile: &Profile,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn delete(
        &self,
        executor: &mut Self::Connection,
        profile_id: &ProfileId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnProfileReadModel: Sync + Send + DependOnDatabaseConnection {
    type ProfileReadModel: ProfileReadModel<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn profile_read_model(&self) -> &Self::ProfileReadModel;
}

/// Read-model query facade for profiles (ADR 0006 decision 3: `*Query` 系統).
///
/// Blanket-implemented for any [`DependOnProfileReadModel`]; obtain it via
/// [`DependOnProfileQuery::profile_query`].
pub trait ProfileQuery: Send + Sync + 'static {
    type Connection: Connection;

    fn find_by_id(
        &self,
        executor: &mut Self::Connection,
        id: &ProfileId,
    ) -> impl Future<Output = error_stack::Result<Option<ProfileProjection>, KernelError>> + Send;

    fn find_by_account_id(
        &self,
        executor: &mut Self::Connection,
        account_id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<Option<ProfileProjection>, KernelError>> + Send;

    fn find_by_account_ids(
        &self,
        executor: &mut Self::Connection,
        account_ids: &[AccountId],
    ) -> impl Future<Output = error_stack::Result<Vec<ProfileProjection>, KernelError>> + Send;
}

impl<T> ProfileQuery for T
where
    T: DependOnProfileReadModel + Send + Sync + 'static,
{
    type Connection =
        <<T as DependOnProfileReadModel>::ProfileReadModel as ProfileReadModel>::Connection;

    async fn find_by_id(
        &self,
        executor: &mut Self::Connection,
        id: &ProfileId,
    ) -> error_stack::Result<Option<ProfileProjection>, KernelError> {
        self.profile_read_model().find_by_id(executor, id).await
    }

    async fn find_by_account_id(
        &self,
        executor: &mut Self::Connection,
        account_id: &AccountId,
    ) -> error_stack::Result<Option<ProfileProjection>, KernelError> {
        self.profile_read_model()
            .find_by_account_id(executor, account_id)
            .await
    }

    async fn find_by_account_ids(
        &self,
        executor: &mut Self::Connection,
        account_ids: &[AccountId],
    ) -> error_stack::Result<Vec<ProfileProjection>, KernelError> {
        self.profile_read_model()
            .find_by_account_ids(executor, account_ids)
            .await
    }
}

pub trait DependOnProfileQuery: DependOnDatabaseConnection + Send + Sync {
    type ProfileQuery: ProfileQuery<
        Connection = <<Self as DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Connection,
    >;
    fn profile_query(&self) -> &Self::ProfileQuery;
}

impl<T> DependOnProfileQuery for T
where
    T: DependOnProfileReadModel + DependOnDatabaseConnection + Send + Sync + 'static,
{
    type ProfileQuery = Self;
    fn profile_query(&self) -> &Self::ProfileQuery {
        self
    }
}
