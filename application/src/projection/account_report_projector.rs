use kernel::interfaces::database::{
    DependOnDatabaseConnection, Savepoint, Transaction, TransactionalDatabaseConnection,
};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::projection::{
    AccountReportEventLog, AccountReportProjectionWriter, DependOnAccountReportEventLog,
    DependOnAccountReportProjectionWriter, DependOnProjectionCheckpointStore,
    ProjectionCheckpointStore,
};
use kernel::interfaces::read_model::{AccountReportReadModel, DependOnAccountReportReadModel};
use kernel::prelude::entity::{AccountReport, AccountReportEvent, AccountReportId, EventEnvelope};
use kernel::KernelError;
use std::collections::HashMap;
use std::future::Future;

pub const ACCOUNT_REPORT_PROJECTOR_NAME: &str = "account_report_projector";

pub const ACCOUNT_REPORT_PROJECTOR_WINDOW: i64 = 100;

pub const ACCOUNT_REPORT_PROJECTOR_BATCH_LIMIT: i64 = 1000;

pub trait ProjectAccountReportBatch:
    DependOnDatabaseConnection<DatabaseConnection: TransactionalDatabaseConnection>
    + DependOnAccountReportEventLog
    + DependOnAccountReportReadModel
    + DependOnProjectionCheckpointStore
    + DependOnAccountReportProjectionWriter
{
    fn project_account_report_batch(
        &self,
    ) -> impl Future<Output = error_stack::Result<i64, KernelError>> + Send + '_ {
        async move {
            let mut transaction = self.database_connection().get_transaction().await?;

            let checkpoint = {
                let executor = transaction.connection();
                self.projection_checkpoint_store()
                    .get(executor, ACCOUNT_REPORT_PROJECTOR_NAME)
                    .await?
                    .unwrap_or(0)
            };
            let events = {
                let executor = transaction.connection();
                self.account_report_event_log()
                    .find_by_seq_window(
                        executor,
                        checkpoint - ACCOUNT_REPORT_PROJECTOR_WINDOW,
                        ACCOUNT_REPORT_PROJECTOR_BATCH_LIMIT,
                    )
                    .await?
            };
            if events.is_empty() {
                transaction.commit().await?;
                return Ok(checkpoint);
            }
            let max_seq = events.last().map(|event| event.seq).unwrap_or(checkpoint);

            let mut groups: HashMap<
                AccountReportId,
                Vec<EventEnvelope<AccountReportEvent, AccountReport>>,
            > = HashMap::new();
            for event in events {
                groups
                    .entry(AccountReportId::new(*event.envelope.id.as_ref()))
                    .or_default()
                    .push(event.envelope);
            }

            for (account_report_id, mut envelopes) in groups {
                envelopes.sort_by_key(|event| *event.version.as_ref());

                let existing = {
                    let executor = transaction.connection();
                    self.account_report_read_model()
                        .find_by_id_unfiltered(executor, &account_report_id)
                        .await?
                };
                let pending: Vec<_> = envelopes
                    .into_iter()
                    .filter(|event| match &existing {
                        Some(account_report) => {
                            *event.version.as_ref() > *account_report.version().as_ref()
                        }
                        None => true,
                    })
                    .collect();
                if pending.is_empty() {
                    continue;
                }

                let mut entity = existing.map(AccountReport::from);
                let mut fold_failed = false;
                for event in pending {
                    if let Err(error) = AccountReport::apply(&mut entity, event) {
                        tracing::warn!(
                            ?error,
                            account_report_id = %account_report_id.as_ref(),
                            "account report projection fold skipped (incomplete stream); window re-read will retry"
                        );
                        fold_failed = true;
                        break;
                    }
                }
                if fold_failed {
                    continue;
                }
                let Some(account_report) = entity else {
                    tracing::warn!(
                        account_report_id = %account_report_id.as_ref(),
                        "account report projection fold produced no entity"
                    );
                    continue;
                };

                let savepoint = transaction.savepoint().await?;
                let write_result = {
                    let executor = transaction.connection();
                    self.account_report_projection_writer()
                        .upsert(executor, &account_report)
                        .await
                };
                match write_result {
                    Ok(()) => {
                        let executor = transaction.connection();
                        savepoint.commit(executor).await?;
                    }
                    Err(error) => {
                        let executor = transaction.connection();
                        savepoint.rollback(executor).await?;
                        tracing::warn!(
                            ?error,
                            account_report_id = %account_report_id.as_ref(),
                            "account report projection skipped"
                        );
                    }
                }
            }

            let executor = transaction.connection();
            self.projection_checkpoint_store()
                .set(executor, ACCOUNT_REPORT_PROJECTOR_NAME, max_seq)
                .await?;
            transaction.commit().await?;
            Ok(max_seq)
        }
    }
}

impl<T> ProjectAccountReportBatch for T where
    T: DependOnDatabaseConnection<DatabaseConnection: TransactionalDatabaseConnection>
        + DependOnAccountReportEventLog
        + DependOnAccountReportReadModel
        + DependOnProjectionCheckpointStore
        + DependOnAccountReportProjectionWriter
{
}
