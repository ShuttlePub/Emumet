use super::database::MockConnection;
use error_stack::Report;
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::read_model::{ProfileProjection, ProfileReadModel};
use kernel::interfaces::repository::{AggregateRepository, Rehydrated};
use kernel::prelude::entity::{
    AccountId, CommandEnvelope, EventEnvelope, EventVersion, ImageId, Nanoid, Profile,
    ProfileEvent, ProfileId,
};
use kernel::KernelError;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct MockProfileRepository {
    pub profile: Option<Profile>,
    pub saved_events: Arc<Mutex<Vec<ProfileEvent>>>,
}

impl AggregateRepository<Profile> for MockProfileRepository {
    type Connection = MockConnection;
    type Id = ProfileId;

    async fn load(
        &self,
        _executor: &mut Self::Connection,
        id: &Self::Id,
    ) -> error_stack::Result<Rehydrated<Profile>, KernelError> {
        self.profile
            .as_ref()
            .filter(|profile| profile.id() == id)
            .map(|profile| Rehydrated::new(profile.clone(), profile.version().clone()))
            .ok_or_else(|| Report::new(KernelError::NotFound))
    }

    async fn save(
        &self,
        _executor: &mut Self::Connection,
        command: CommandEnvelope<ProfileEvent, Profile>,
    ) -> error_stack::Result<EventEnvelope<ProfileEvent, Profile>, KernelError> {
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
pub struct MockProfileReadModel {
    pub profiles: Arc<Mutex<Vec<ProfileProjection>>>,
    pub updated: Arc<Mutex<Vec<ProfileProjection>>>,
}

impl ProfileReadModel for MockProfileReadModel {
    type Connection = MockConnection;

    async fn find_by_id(
        &self,
        _executor: &mut Self::Connection,
        id: &ProfileId,
    ) -> error_stack::Result<Option<ProfileProjection>, KernelError> {
        Ok(self
            .profiles
            .lock()
            .unwrap()
            .iter()
            .find(|profile| profile.id() == id)
            .cloned())
    }

    async fn find_by_id_unfiltered(
        &self,
        executor: &mut Self::Connection,
        id: &ProfileId,
    ) -> error_stack::Result<Option<ProfileProjection>, KernelError> {
        self.find_by_id(executor, id).await
    }

    async fn find_by_account_id(
        &self,
        _executor: &mut Self::Connection,
        account_id: &AccountId,
    ) -> error_stack::Result<Option<ProfileProjection>, KernelError> {
        Ok(self
            .profiles
            .lock()
            .unwrap()
            .iter()
            .find(|profile| profile.account_id() == account_id)
            .cloned())
    }

    async fn find_by_account_ids(
        &self,
        _executor: &mut Self::Connection,
        account_ids: &[AccountId],
    ) -> error_stack::Result<Vec<ProfileProjection>, KernelError> {
        Ok(self
            .profiles
            .lock()
            .unwrap()
            .iter()
            .filter(|profile| account_ids.contains(profile.account_id()))
            .cloned()
            .collect())
    }

    async fn find_by_nanoid(
        &self,
        _executor: &mut Self::Connection,
        nanoid: &Nanoid<Profile>,
    ) -> error_stack::Result<Option<ProfileProjection>, KernelError> {
        Ok(self
            .profiles
            .lock()
            .unwrap()
            .iter()
            .find(|profile| profile.nanoid() == nanoid)
            .cloned())
    }

    async fn create(
        &self,
        _executor: &mut Self::Connection,
        _profile: &Profile,
    ) -> error_stack::Result<(), KernelError> {
        Ok(())
    }

    async fn update(
        &self,
        _executor: &mut Self::Connection,
        profile: &Profile,
    ) -> error_stack::Result<(), KernelError> {
        let projection: ProfileProjection = profile.clone().into();
        let mut profiles = self.profiles.lock().unwrap();
        if let Some(existing) = profiles.iter_mut().find(|item| item.id() == profile.id()) {
            *existing = projection.clone();
        }
        self.updated.lock().unwrap().push(projection);
        Ok(())
    }

    async fn delete(
        &self,
        _executor: &mut Self::Connection,
        _profile_id: &ProfileId,
    ) -> error_stack::Result<(), KernelError> {
        Ok(())
    }
}

pub fn profile_with_media(
    profile_id: ProfileId,
    account_id: AccountId,
    nanoid: Nanoid<Profile>,
    icon: ImageId,
    banner: ImageId,
) -> Profile {
    let command = Profile::create(
        profile_id,
        account_id,
        None,
        None,
        Some(icon),
        Some(banner),
        nanoid,
    );
    let mut profile = None;
    Profile::apply(
        &mut profile,
        EventEnvelope::new(
            command.id().clone(),
            command.event().clone(),
            EventVersion::default(),
        ),
    )
    .unwrap();
    profile.unwrap()
}
