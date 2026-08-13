use error_stack::Report;
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::event_store::{AccountEventStore, DependOnAccountEventStore};
use kernel::interfaces::repository::Rehydrated;
use kernel::prelude::entity::{Account, AccountId, EventId};
use kernel::KernelError;

pub(crate) async fn rehydrate_account<T>(
    deps: &T,
    executor: &mut <<T as kernel::interfaces::database::DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Connection,
    account_id: &AccountId,
) -> error_stack::Result<Rehydrated<Account>, KernelError>
where
    T: DependOnAccountEventStore + ?Sized,
{
    let event_id = EventId::from(account_id.clone());
    let events = deps
        .account_event_store()
        .find_by_id(executor, &event_id, None)
        .await?;
    Rehydrated::from_events(events)?.ok_or_else(|| {
        Report::new(KernelError::NotFound).attach_printable(format!(
            "No events found for account: {}",
            account_id.as_ref()
        ))
    })
}
