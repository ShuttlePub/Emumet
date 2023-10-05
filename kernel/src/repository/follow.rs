use crate::{
    entity::{Follow, FollowAccount, Id},
    error::KernelError,
};

pub trait FollowRepository: 'static + Sync + Send {
    fn find_by_id(&self, id: &Id<Follow>) -> Result<Option<Follow>, KernelError>;
    fn find_by_soruce_id(&self, id: &FollowAccount) -> Result<Vec<Follow>, KernelError>;
    fn find_by_target_id(&self, id: &FollowAccount) -> Result<Vec<Follow>, KernelError>;
    fn save(&self, follow: &Follow) -> Result<(), KernelError>;
    fn update(&self, follow: &Follow) -> Result<(), KernelError>;
    fn delete(&self, follow: &Follow) -> Result<(), KernelError>;
}

pub trait DependOnFollowRepository: 'static + Sync + Send {
    type FollowRepository: FollowRepository;

    fn follow_repository(&self) -> &Self::FollowRepository;
}
