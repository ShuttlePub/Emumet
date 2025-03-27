use crate::handler::AppModule;
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
use uuid::Uuid;

struct AccountApplier(RedisMessageQueue<AppModule, Uuid, AccountId>);

impl AccountApplier {
    fn new(module: AppModule) -> Self {
        let queue = RedisMessageQueue::new(
            module.handler().redis().pool().clone(),
            module.clone(),
            "account_applier".to_string(),
            MQConfig::default(),
            Uuid::new_v4,
            |module: AppModule, id: AccountId| async move {
                module
                    .handler()
                    .pgpool()
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
