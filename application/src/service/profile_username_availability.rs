//! Profile username availability check (advisory, read-only).
//!
//! The format and reserved-word rules are exactly the ones the creation / assignment use cases
//! apply (`ProfileUsername::check`). Whether a username is already taken is answered from the
//! append-only `profile_events` table, the same source as the unique index that rejects a
//! duplicate with `Concurrency` (409). It therefore also covers usernames of Profiles that were
//! deleted: a username is never reused.
//!
//! The result is only a hint: another request may take the username after this read, so the
//! creation / assignment API stays the final authority.

use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::event_store::{DependOnProfileEventStore, ProfileEventStore};
use kernel::prelude::entity::{ProfileUsername, ProfileUsernameError};
use kernel::KernelError;
use std::future::Future;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileUsernameAvailability {
    Available,
    InvalidFormat,
    Reserved,
    /// In use, or used by a Profile that no longer exists. The two are deliberately not told apart.
    Taken,
}

pub trait CheckProfileUsernameAvailabilityUseCase:
    'static + Sync + Send + DependOnDatabaseConnection + DependOnProfileEventStore
{
    /// Never writes. Malformed and reserved values are answered without touching the database.
    fn check_profile_username_availability<'a>(
        &'a self,
        username: &'a str,
    ) -> impl Future<Output = error_stack::Result<ProfileUsernameAvailability, KernelError>> + Send + 'a
    {
        async move {
            match ProfileUsername::check(username) {
                Err(ProfileUsernameError::Format) => {
                    return Ok(ProfileUsernameAvailability::InvalidFormat)
                }
                Err(ProfileUsernameError::Reserved) => {
                    return Ok(ProfileUsernameAvailability::Reserved)
                }
                Ok(()) => {}
            }
            let username = ProfileUsername::new(username.to_string());
            let mut connection = self.database_connection().connection().await?;
            let taken = self
                .profile_event_store()
                .username_exists(&mut connection, &username)
                .await?;
            Ok(if taken {
                ProfileUsernameAvailability::Taken
            } else {
                ProfileUsernameAvailability::Available
            })
        }
    }
}

impl<T> CheckProfileUsernameAvailabilityUseCase for T where
    T: 'static + Sync + Send + DependOnDatabaseConnection + DependOnProfileEventStore
{
}
