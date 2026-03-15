use error_stack::Report;
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection, Executor};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::event_store::{DependOnProfileEventStore, ProfileEventStore};
use kernel::interfaces::read_model::{DependOnProfileReadModel, ProfileReadModel};
use kernel::interfaces::signal::Signal;
use kernel::prelude::entity::{
    AccountId, EventVersion, FieldAction, ImageId, Nanoid, Profile, ProfileDisplayName, ProfileId,
    ProfileSummary,
};
use kernel::KernelError;
use std::future::Future;

// --- Signal DI trait (adapter-specific) ---

pub trait DependOnProfileSignal: Send + Sync {
    type ProfileSignal: Signal<ProfileId> + Send + Sync + 'static;
    fn profile_signal(&self) -> &Self::ProfileSignal;
}

// --- Param structs ---

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
    pub current_version: EventVersion<Profile>,
}

// --- ProfileCommandProcessor ---

pub trait ProfileCommandProcessor: Send + Sync + 'static {
    type Executor: Executor;

    fn create(
        &self,
        executor: &mut Self::Executor,
        param: CreateProfileParam,
    ) -> impl Future<Output = error_stack::Result<Profile, KernelError>> + Send;

    fn update(
        &self,
        executor: &mut Self::Executor,
        param: UpdateProfileParam,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

impl<T> ProfileCommandProcessor for T
where
    T: DependOnProfileEventStore + DependOnProfileSignal + Send + Sync + 'static,
{
    type Executor =
        <<T as DependOnProfileEventStore>::ProfileEventStore as ProfileEventStore>::Executor;

    async fn create(
        &self,
        executor: &mut Self::Executor,
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

        let event_envelope = self
            .profile_event_store()
            .persist_and_transform(executor, command)
            .await?;

        let mut profile = None;
        Profile::apply(&mut profile, event_envelope)?;
        let profile = profile.ok_or_else(|| {
            Report::new(KernelError::Internal)
                .attach_printable("Failed to construct profile from created event")
        })?;

        if let Err(e) = self.profile_signal().emit(profile_id).await {
            tracing::error!(?e, "Failed to emit profile signal");
        }

        Ok(profile)
    }

    async fn update(
        &self,
        executor: &mut Self::Executor,
        param: UpdateProfileParam,
    ) -> error_stack::Result<(), KernelError> {
        let command = Profile::update(
            param.profile_id.clone(),
            param.display_name,
            param.summary,
            param.icon,
            param.banner,
            param.current_version,
        );

        self.profile_event_store()
            .persist_and_transform(executor, command)
            .await?;

        if let Err(e) = self.profile_signal().emit(param.profile_id).await {
            tracing::error!(?e, "Failed to emit profile signal");
        }

        Ok(())
    }
}

pub trait DependOnProfileCommandProcessor: DependOnDatabaseConnection + Send + Sync {
    type ProfileCommandProcessor: ProfileCommandProcessor<
        Executor = <<Self as DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Executor,
    >;
    fn profile_command_processor(&self) -> &Self::ProfileCommandProcessor;
}

impl<T> DependOnProfileCommandProcessor for T
where
    T: DependOnProfileEventStore
        + DependOnProfileSignal
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

// --- ProfileQueryProcessor ---

pub trait ProfileQueryProcessor: Send + Sync + 'static {
    type Executor: Executor;

    fn find_by_id(
        &self,
        executor: &mut Self::Executor,
        id: &ProfileId,
    ) -> impl Future<Output = error_stack::Result<Option<Profile>, KernelError>> + Send;

    fn find_by_account_id(
        &self,
        executor: &mut Self::Executor,
        account_id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<Option<Profile>, KernelError>> + Send;

    fn find_by_account_ids(
        &self,
        executor: &mut Self::Executor,
        account_ids: &[AccountId],
    ) -> impl Future<Output = error_stack::Result<Vec<Profile>, KernelError>> + Send;
}

impl<T> ProfileQueryProcessor for T
where
    T: DependOnProfileReadModel + Send + Sync + 'static,
{
    type Executor =
        <<T as DependOnProfileReadModel>::ProfileReadModel as ProfileReadModel>::Executor;

    async fn find_by_id(
        &self,
        executor: &mut Self::Executor,
        id: &ProfileId,
    ) -> error_stack::Result<Option<Profile>, KernelError> {
        self.profile_read_model().find_by_id(executor, id).await
    }

    async fn find_by_account_id(
        &self,
        executor: &mut Self::Executor,
        account_id: &AccountId,
    ) -> error_stack::Result<Option<Profile>, KernelError> {
        self.profile_read_model()
            .find_by_account_id(executor, account_id)
            .await
    }

    async fn find_by_account_ids(
        &self,
        executor: &mut Self::Executor,
        account_ids: &[AccountId],
    ) -> error_stack::Result<Vec<Profile>, KernelError> {
        self.profile_read_model()
            .find_by_account_ids(executor, account_ids)
            .await
    }
}

pub trait DependOnProfileQueryProcessor: DependOnDatabaseConnection + Send + Sync {
    type ProfileQueryProcessor: ProfileQueryProcessor<
        Executor = <<Self as DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Executor,
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
