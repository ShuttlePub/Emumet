use crate::{entity::{Profile, Account, Id}, error::KernelError};

pub trait ProfileRepository: 'static + Sync + Send {
    fn find_by_id(&self, id: &Id<Account>) -> Result<Option<Profile>, KernelError>;
    fn save(&self, profile: &Profile) -> Result<(), KernelError>;
    fn update(&self, profile: &Profile) -> Result<(), KernelError>;
    fn delete(&self, profile: &Profile) -> Result<(), KernelError>;
}

pub trait DependOnProfileRepository: 'static + Sync + Send {
    type ProfileRepository: ProfileRepository;

    fn profile_repository(&self) -> &Self::ProfileRepository;
}
