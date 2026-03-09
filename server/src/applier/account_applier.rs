use crate::handler::Handler;
use application::service::account::UpdateAccountService;
use error_stack::ResultExt;
use kernel::interfaces::signal::Signal;
use kernel::prelude::entity::AccountId;
use kernel::KernelError;
use rikka_mq::config::MQConfig;
use rikka_mq::define::redis::mq::RedisMessageQueue;
use rikka_mq::error::ErrorOperation;
use rikka_mq::info::QueueInfo;
use rikka_mq::mq::MessageQueue;
use std::sync::Arc;
use uuid::Uuid;

pub(crate) struct AccountApplier(RedisMessageQueue<Arc<Handler>, Uuid, AccountId>);

impl AccountApplier {
    pub fn new(handler: Arc<Handler>) -> Self {
        let queue = RedisMessageQueue::new(
            handler.redis().pool().clone(),
            handler,
            "account_applier".to_string(),
            MQConfig::default(),
            Uuid::new_v4,
            |handler: Arc<Handler>, id: AccountId| async move {
                handler
                    .as_ref()
                    .update_account(id)
                    .await
                    .map_err(|e| ErrorOperation::Delay(format!("{:?}", e)))
            },
        );
        AccountApplier(queue)
    }
}

impl Signal<AccountId> for AccountApplier {
    async fn emit(&self, signal_id: AccountId) -> error_stack::Result<(), KernelError> {
        self.0
            .queue(QueueInfo::new(Uuid::new_v4(), signal_id))
            .await
            .map_err(|e| error_stack::Report::new(e))
            .change_context_lazy(|| KernelError::Internal)
    }
}
