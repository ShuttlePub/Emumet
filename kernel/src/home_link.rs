//! P1 home-link ledger and transaction participation ports. No HTTP/wait-for-delivery API.
use crate::database::{DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::{Account, AccountId, AuthAccountId, EventVersion, ProfileId};
use crate::KernelError;
use std::future::Future;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HomeLink {
    pub id: String,
    pub profile_id: ProfileId,
    pub actor: String,
    pub origin: String,
    pub trust_mode: i16,
    pub generation: i64,
    pub state: String,
    pub created_at: OffsetDateTime,
    pub unlinked_at: Option<OffsetDateTime>,
    pub unlink_reason: Option<String>,
}

/// One authoritative, gate-protected write-side owner snapshot.
#[derive(Debug, Clone)]
pub struct LinkOwner {
    pub profile_id: ProfileId,
    pub account_id: AccountId,
    pub account_version: EventVersion<Account>,
    pub active: bool,
}

/// Account gates ascending -> Profile gates/rows ascending -> relation locks -> link IDs
/// ascending -> global/profile/origin quotas -> jobs/quarantine -> attempts/audits.
/// Only transaction-scoped gates; owner candidates changing require a fresh transaction.
pub trait HomeLinkStore: Send + Sync + 'static {
    type Connection: crate::database::Connection;
    fn lock_accounts(
        &self,
        connection: &mut Self::Connection,
        ids: &[AccountId],
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
    fn lock_profiles(
        &self,
        connection: &mut Self::Connection,
        ids: &[ProfileId],
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
    /// Requires the Account gate. Locks, in one ascending Profile-gate pass followed by one
    /// ascending relationship-gate pass, the union of the Account's live Profiles, its ledger-only
    /// Profile IDs and `extra` (e.g. every Profile stream a reactivation may restore). The
    /// lifecycle projector calls this once **before** any deactivation/restoration so later
    /// per-step locks only re-enter locks that are already held.
    fn lock_account_profile_set(
        &self,
        connection: &mut Self::Connection,
        account: &AccountId,
        extra: &[ProfileId],
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
    fn owner_candidate(
        &self,
        connection: &mut Self::Connection,
        nanoid: &str,
    ) -> impl Future<Output = error_stack::Result<(ProfileId, AccountId), KernelError>> + Send;
    /// Requires both gates. Returns current event-sourced Account state, never a projection.
    fn owner(
        &self,
        connection: &mut Self::Connection,
        profile: &ProfileId,
        candidate: &AccountId,
    ) -> impl Future<Output = error_stack::Result<LinkOwner, KernelError>> + Send;
    fn account_snapshot(
        &self,
        connection: &mut Self::Connection,
        account: &AccountId,
    ) -> impl Future<Output = error_stack::Result<Option<Account>, KernelError>> + Send;
    fn caller_owns(
        &self,
        connection: &mut Self::Connection,
        owner: &AccountId,
        auth: &AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<bool, KernelError>> + Send;
    fn delete_owned_profiles(
        &self,
        connection: &mut Self::Connection,
        account: &AccountId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
    fn transfer_owner(
        &self,
        connection: &mut Self::Connection,
        profile: &ProfileId,
        to: &AccountId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
    fn list(
        &self,
        connection: &mut Self::Connection,
        profile: &ProfileId,
    ) -> impl Future<Output = error_stack::Result<Vec<HomeLink>, KernelError>> + Send;
    fn register(
        &self,
        connection: &mut Self::Connection,
        owner: &LinkOwner,
        actor: &str,
        origin: &str,
    ) -> impl Future<Output = error_stack::Result<(HomeLink, bool), KernelError>> + Send;
    fn unlink(
        &self,
        connection: &mut Self::Connection,
        profile: &ProfileId,
        id: &str,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
    /// All links, including Profiles no longer materialized. Only links whose registration
    /// observed an Account version older than this deactivation are affected (replay fencing).
    fn deactivate(
        &self,
        connection: &mut Self::Connection,
        account: &AccountId,
        version: &EventVersion<Account>,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}
pub trait DependOnHomeLinkStore: DependOnDatabaseConnection {
    type HomeLinkStore: HomeLinkStore<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;
    fn home_link_store(&self) -> &Self::HomeLinkStore;
}

/// Q1 plugs DB-only cancellation/capacity release into this port. Called on the SAME
/// transaction, AFTER ordered link locks, BEFORE commit. Must not do HTTP/external I/O.
/// The unlinked generation passed here is the generation that was active, not its successor.
pub trait LinkCancellationParticipant: Send + Sync {
    type Connection: crate::database::Connection;
    fn cancel_link_generation(
        &self,
        connection: &mut Self::Connection,
        link_id: &str,
        generation: i64,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

/// Registration resolves/validates outside the gate. S1 must re-resolve, validate and pin
/// the actual HTTP operation too; registration resolution is not permanent SSRF approval.
pub trait HomeOriginPolicy: Send + Sync + 'static {
    fn validate(
        &self,
        origin: &str,
    ) -> impl Future<Output = error_stack::Result<String, KernelError>> + Send;
}
pub trait DependOnHomeOriginPolicy: Send + Sync {
    type HomeOriginPolicy: HomeOriginPolicy;
    fn home_origin_policy(&self) -> &Self::HomeOriginPolicy;
}
