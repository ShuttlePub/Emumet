use crate::handler::Handler;
use error_stack::ResultExt;
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::event_store::{AccountEventStore, DependOnAccountEventStore};
use kernel::interfaces::read_model::{AccountReadModel, DependOnAccountReadModel};
use kernel::interfaces::signal::Signal;
use kernel::prelude::entity::{Account, AccountEvent, AccountId, EventId};
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
                let mut tx = handler
                    .database_connection()
                    .begin_transaction()
                    .await
                    .map_err(|e| ErrorOperation::Delay(format!("{:?}", e)))?;
                let event_id = EventId::from(id.clone());

                // 既存Projection取得
                let existing = handler
                    .account_read_model()
                    .find_by_id(&mut tx, &id)
                    .await
                    .map_err(|e| ErrorOperation::Delay(format!("{:?}", e)))?;
                let since_version = existing.as_ref().map(|a| a.version().clone());

                // 新規イベント取得
                let events = handler
                    .account_event_store()
                    .find_by_id(&mut tx, &event_id, since_version.as_ref())
                    .await
                    .map_err(|e| ErrorOperation::Delay(format!("{:?}", e)))?;
                if events.is_empty() {
                    return Ok(());
                }

                // Created イベントから auth_account_id 抽出
                let mut auth_account_id_for_link = None;
                for event in &events {
                    if let AccountEvent::Created {
                        auth_account_id, ..
                    } = &event.event
                    {
                        auth_account_id_for_link = Some(auth_account_id.clone());
                    }
                }

                // イベント適用
                let mut entity = existing;
                for event in events {
                    Account::apply(&mut entity, event)
                        .map_err(|e| ErrorOperation::Delay(format!("{:?}", e)))?;
                }

                // Projection更新
                match (&entity, &since_version) {
                    (Some(account), None) => {
                        handler
                            .account_read_model()
                            .create(&mut tx, account)
                            .await
                            .map_err(|e| ErrorOperation::Delay(format!("{:?}", e)))?;
                        if let Some(auth_id) = auth_account_id_for_link {
                            handler
                                .account_read_model()
                                .link_auth_account(&mut tx, &id, &auth_id)
                                .await
                                .map_err(|e| ErrorOperation::Delay(format!("{:?}", e)))?;
                        }
                    }
                    (Some(account), Some(_)) => {
                        handler
                            .account_read_model()
                            .update(&mut tx, account)
                            .await
                            .map_err(|e| ErrorOperation::Delay(format!("{:?}", e)))?;
                    }
                    (None, Some(_)) => {
                        handler
                            .account_read_model()
                            .delete(&mut tx, &id)
                            .await
                            .map_err(|e| ErrorOperation::Delay(format!("{:?}", e)))?;
                    }
                    (None, None) => {
                        tracing::warn!(
                            "Account applier: entity is None with no prior projection for id {:?}",
                            id
                        );
                    }
                }
                Ok(())
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
