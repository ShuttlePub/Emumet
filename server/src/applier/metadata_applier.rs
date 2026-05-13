use crate::handler::Handler;
use application::service::metadata::UpdateMetadata;
use error_stack::ResultExt;
use kernel::interfaces::signal::Signal;
use kernel::prelude::entity::MetadataId;
use rikka_mq::define::redis::mq::RedisMessageQueue;
use rikka_mq::mq::MessageQueue;
use std::sync::Arc;
use uuid::Uuid;

pub(crate) struct MetadataApplier(RedisMessageQueue<Arc<Handler>, Uuid, MetadataId>);

impl MetadataApplier {
    pub fn new(handler: Arc<Handler>) -> Self {
        let queue = RedisMessageQueue::new(
            handler.redis().pool().clone(),
            handler,
            "metadata_applier".to_string(),
            rikka_mq::config::MQConfig::default(),
            Uuid::new_v4,
            |handler: Arc<Handler>, id: MetadataId| async move {
                handler
                    .pgpool()
                    .update_metadata(id)
                    .await
                    .map_err(|e| rikka_mq::error::ErrorOperation::Delay(format!("{:?}", e)))
            },
        );
        queue.start_workers();
        MetadataApplier(queue)
    }
}

impl Signal<MetadataId> for MetadataApplier {
    async fn emit(&self, signal_id: MetadataId) -> error_stack::Result<(), kernel::KernelError> {
        self.0
            .queue(rikka_mq::info::QueueInfo::new(Uuid::new_v4(), signal_id))
            .await
            .map_err(|e| error_stack::Report::new(e))
            .change_context_lazy(|| kernel::KernelError::Internal)
    }
}
