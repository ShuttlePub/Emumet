//! Whether the Account that currently owns a Profile may take part in federation.
//!
//! A Profile is an actor only while its owner Account is active: not deactivated, not banned and
//! not under an unexpired suspension (the same rule `AccountQuery::find_by_id` applies, which is
//! what the legacy Account actor relied on). A suspended or banned Account's Profile keeps its
//! identity and key, but is neither published (actor / WebFinger), nor addressable (inbox), nor
//! allowed to start a Follow, nor sends queued activities; the moment the restriction ends all of
//! it works again with the same actor URL and key.

use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::read_model::{AccountQuery, DependOnAccountQuery};
use kernel::prelude::entity::AccountId;
use kernel::KernelError;

pub(crate) async fn owner_account_is_active<T>(
    deps: &T,
    executor: &mut <<T as DependOnDatabaseConnection>::DatabaseConnection as DatabaseConnection>::Connection,
    owner: &AccountId,
) -> error_stack::Result<bool, KernelError>
where
    T: ?Sized + DependOnAccountQuery + DependOnDatabaseConnection,
{
    Ok(deps
        .account_query()
        .find_by_id(executor, owner)
        .await?
        .is_some())
}
