use crate::{entity::{Profile, Account, Id}, error::KernelError};

#[async_trait::async_trait]
pub trait ProfileRepository: 'static + Sync + Send {
    async fn find_by_id(&self, id: &Id<Account>) -> Result<Option<Profile>, KernelError>;
    async fn save(&self, profile: &Profile) -> Result<(), KernelError>;
    async fn update(&self, profile: &Profile) -> Result<(), KernelError>;
    async fn delete(&self, profile: &Profile) -> Result<(), KernelError>;
}

pub trait DependOnProfileRepository: 'static + Sync + Send {
    type ProfileRepository: ProfileRepository;

    fn profile_repository(&self) -> &Self::ProfileRepository;
}
