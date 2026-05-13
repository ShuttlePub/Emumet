use crate::handler::Handler;
use application::service::auth_account::UpdateAuthAccount;
use error_stack::ResultExt;
use kernel::interfaces::signal::Signal;
use kernel::prelude::entity::AuthAccountId;
use rikka_mq::define::redis::mq::RedisMessageQueue;
use rikka_mq::mq::MessageQueue;
use std::sync::Arc;
use uuid::Uuid;

pub(crate) struct AuthAccountApplier(RedisMessageQueue<Arc<Handler>, Uuid, AuthAccountId>);

impl AuthAccountApplier {
    pub fn new(handler: Arc<Handler>) -> Self {
        let queue = RedisMessageQueue::new(
            handler.redis().pool().clone(),
            handler,
            "auth_account_applier".to_string(),
            rikka_mq::config::MQConfig::default(),
            Uuid::new_v4,
            |handler: Arc<Handler>, id: AuthAccountId| async move {
                handler
                    .pgpool()
                    .update_auth_account(id)
                    .await
                    .map_err(|e| rikka_mq::error::ErrorOperation::Delay(format!("{:?}", e)))
            },
        );
        queue.start_workers();
        AuthAccountApplier(queue)
    }
}

impl Signal<AuthAccountId> for AuthAccountApplier {
    async fn emit(&self, signal_id: AuthAccountId) -> error_stack::Result<(), kernel::KernelError> {
        self.0
            .queue(rikka_mq::info::QueueInfo::new(Uuid::new_v4(), signal_id))
            .await
            .map_err(|e| error_stack::Report::new(e))
            .change_context_lazy(|| kernel::KernelError::Internal)
    }
}
