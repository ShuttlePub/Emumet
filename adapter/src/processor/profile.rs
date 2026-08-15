use error_stack::Report;
use kernel::interfaces::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::read_model::{
    DependOnProfileReadModel, ProfileProjection, ProfileReadModel,
};
use kernel::interfaces::repository::{AggregateRepository, DependOnProfileRepository};
use kernel::prelude::entity::{
    AccountId, FieldAction, ImageId, Nanoid, Profile, ProfileDisplayName, ProfileId, ProfileSummary,
};
use kernel::KernelError;
use std::future::Future;

#[derive(Debug)]
pub struct CreateProfileParam {
    pub account_id: AccountId,
    pub display_name: Option<ProfileDisplayName>,
    pub summary: Option<ProfileSummary>,
    pub icon: Option<ImageId>,
    pub banner: Option<ImageId>,
    pub nano_id: Nanoid<Profile>,
}

#[derive(Debug)]
pub struct UpdateProfileParam {
    pub profile_id: ProfileId,
    pub display_name: FieldAction<ProfileDisplayName>,
    pub summary: FieldAction<ProfileSummary>,
    pub icon: FieldAction<ImageId>,
    pub banner: FieldAction<ImageId>,
}

pub trait ProfileCommandProcessor: Send + Sync + 'static {
    type Connection: Connection;

    fn create(
        &self,
        executor: &mut Self::Connection,
        param: CreateProfileParam,
    ) -> impl Future<Output = error_stack::Result<Profile, KernelError>> + Send;

    fn update(
        &self,
        executor: &mut Self::Connection,
        param: UpdateProfileParam,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

impl<T> ProfileCommandProcessor for T
where
    T: DependOnProfileRepository + DependOnProfileReadModel + Send + Sync + 'static,
{
    type Connection =
        <<T as DependOnProfileRepository>::ProfileRepository as AggregateRepository<Profile>>::Connection;

    async fn create(
        &self,
        executor: &mut Self::Connection,
        param: CreateProfileParam,
    ) -> error_stack::Result<Profile, KernelError> {
        let profile_id = ProfileId::new(kernel::generate_id());
        let command = Profile::create(
            profile_id.clone(),
            param.account_id,
            param.display_name,
            param.summary,
            param.icon,
            param.banner,
            param.nano_id,
        );

        let event_envelope = self.profile_repository().save(executor, command).await?;

        let mut profile = None;
        Profile::apply(&mut profile, event_envelope)?;
        let profile = profile.ok_or_else(|| {
            Report::new(KernelError::Internal)
                .attach_printable("Failed to construct profile from created event")
        })?;

        if let Err(e) = self.profile_read_model().create(executor, &profile).await {
            tracing::error!(?e, "Failed to create profile read model");
            return Err(e);
        }

        Ok(profile)
    }

    async fn update(
        &self,
        executor: &mut Self::Connection,
        param: UpdateProfileParam,
    ) -> error_stack::Result<(), KernelError> {
        let command = Profile::update(
            param.profile_id.clone(),
            param.display_name,
            param.summary,
            param.icon,
            param.banner,
        );

        self.profile_repository().save(executor, command).await?;
        Ok(())
    }
}

pub trait DependOnProfileCommandProcessor: DependOnDatabaseConnection + Send + Sync {
    type ProfileCommandProcessor: ProfileCommandProcessor<
        Connection = <<Self as DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Connection,
    >;
    fn profile_command_processor(&self) -> &Self::ProfileCommandProcessor;
}

impl<T> DependOnProfileCommandProcessor for T
where
    T: DependOnProfileRepository
        + DependOnProfileReadModel
        + DependOnDatabaseConnection
        + Send
        + Sync
        + 'static,
{
    type ProfileCommandProcessor = Self;
    fn profile_command_processor(&self) -> &Self::ProfileCommandProcessor {
        self
    }
}

pub trait ProfileQueryProcessor: Send + Sync + 'static {
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

impl<T> ProfileQueryProcessor for T
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

pub trait DependOnProfileQueryProcessor: DependOnDatabaseConnection + Send + Sync {
    type ProfileQueryProcessor: ProfileQueryProcessor<
        Connection = <<Self as DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Connection,
    >;
    fn profile_query_processor(&self) -> &Self::ProfileQueryProcessor;
}

impl<T> DependOnProfileQueryProcessor for T
where
    T: DependOnProfileReadModel + DependOnDatabaseConnection + Send + Sync + 'static,
{
    type ProfileQueryProcessor = Self;
    fn profile_query_processor(&self) -> &Self::ProfileQueryProcessor {
        self
    }
}
