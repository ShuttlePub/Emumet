use error_stack::Report;
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::event_store::{AccountEventStore, DependOnAccountEventStore};
use kernel::prelude::entity::{Account, AccountId, EventId, EventVersion};
use kernel::KernelError;

pub(crate) async fn rehydrate_account<T>(
    deps: &T,
    executor: &mut <<T as kernel::interfaces::database::DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Executor,
    account_id: &AccountId,
) -> error_stack::Result<(Account, EventVersion<Account>), KernelError>
where
    T: DependOnAccountEventStore + ?Sized,
{
    let event_id = EventId::from(account_id.clone());
    let events = deps
        .account_event_store()
        .find_by_id(executor, &event_id, None)
        .await?;
    if events.is_empty() {
        return Err(Report::new(KernelError::NotFound).attach_printable(format!(
            "No events found for account: {}",
            account_id.as_ref()
        )));
    }
    let mut account: Option<Account> = None;
    for event in events {
        Account::apply(&mut account, event)?;
    }
    let account = account.ok_or_else(|| {
        Report::new(KernelError::NotFound).attach_printable(format!(
            "Account aggregate could not be reconstructed for: {}",
            account_id.as_ref()
        ))
    })?;
    let current_version = account.version().clone();
    Ok((account, current_version))
}
