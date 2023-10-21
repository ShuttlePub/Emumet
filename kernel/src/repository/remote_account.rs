use crate::entity::{RemoteAccount, RemoteAccountId, RemoteAccountUrl};
use crate::KernelError;

#[async_trait::async_trait]
pub trait RemoteAccountRepository: 'static + Sync + Send {
    async fn find_by_id(&self, id: &RemoteAccountId) -> Result<Option<RemoteAccount>, KernelError>;
    async fn find_by_url(
        &self,
        url: &RemoteAccountUrl,
    ) -> Result<Option<RemoteAccount>, KernelError>;
    async fn save(&self, remote_account: &RemoteAccount) -> Result<(), KernelError>;
    async fn delete(&self, remote_account: &RemoteAccount) -> Result<(), KernelError>;
}

pub trait DependOnRemoteAccountRepository: 'static + Sync + Send {
    type RemoteAccountRepository: RemoteAccountRepository;

    fn remote_account_repository(&self) -> &Self::RemoteAccountRepository;
}
