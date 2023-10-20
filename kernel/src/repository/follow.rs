use crate::entity::FollowId;
use crate::{
    entity::{Follow, FollowAccountId},
    error::KernelError,
};

#[async_trait::async_trait]
pub trait FollowRepository: 'static + Sync + Send {
    async fn find_by_id(&self, id: &FollowId) -> Result<Option<Follow>, KernelError>;
    async fn find_by_source_id(&self, id: &FollowAccountId) -> Result<Vec<Follow>, KernelError>;
    async fn find_by_target_id(&self, id: &FollowAccountId) -> Result<Vec<Follow>, KernelError>;
    async fn save(&self, follow: &Follow) -> Result<(), KernelError>;
    async fn update(&self, follow: &Follow) -> Result<(), KernelError>;
    async fn delete(&self, follow: &FollowId) -> Result<(), KernelError>;
}

pub trait DependOnFollowRepository: 'static + Sync + Send {
    type FollowRepository: FollowRepository;

    fn follow_repository(&self) -> &Self::FollowRepository;
}
