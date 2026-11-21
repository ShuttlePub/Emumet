//! Shared transaction gate and durable link ledger. See docs/home-links.md.
use super::account_repository::PostgresAccountRepository;
use super::profile_relationship_lock::PostgresProfileRelationshipLock;
use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use error_stack::Report;
use kernel::interfaces::home_link::{
    DependOnHomeLinkStore, HomeLink, HomeLinkStore, LinkCancellationParticipant, LinkOwner,
};
use kernel::interfaces::repository::AggregateRepository;
use kernel::interfaces::repository::ProfileRelationshipLock;
use kernel::prelude::entity::{
    Account, AccountId, AccountStatus, AuthAccountId, EventVersion, ProfileId,
};
use kernel::KernelError;
use time::OffsetDateTime;

pub struct PostgresHomeLinkStore;

/// Q1's DB-only transaction participant. No queue exists in L1. Q1 replaces this
/// implementation with cancellation/quarantine deletion/capacity release, not a post-commit task.
pub struct PostgresLinkCancellation;
impl LinkCancellationParticipant for PostgresLinkCancellation {
    type Connection = PostgresConnection;
    async fn cancel_link_generation(
        &self,
        _connection: &mut Self::Connection,
        _link_id: &str,
        _generation: i64,
    ) -> error_stack::Result<(), KernelError> {
        Ok(())
    }
}

#[derive(sqlx::FromRow)]
struct LinkRow {
    id: String,
    profile_id: i64,
    actor_snapshot: String,
    origin: String,
    trust_mode: i16,
    generation: i64,
    state: String,
    created_at: OffsetDateTime,
    unlinked_at: Option<OffsetDateTime>,
    unlink_reason: Option<String>,
}
impl From<LinkRow> for HomeLink {
    fn from(r: LinkRow) -> Self {
        Self {
            id: r.id,
            profile_id: ProfileId::new(r.profile_id),
            actor: r.actor_snapshot,
            origin: r.origin,
            trust_mode: r.trust_mode,
            generation: r.generation,
            state: r.state,
            created_at: r.created_at,
            unlinked_at: r.unlinked_at,
            unlink_reason: r.unlink_reason,
        }
    }
}
impl HomeLinkStore for PostgresHomeLinkStore {
    type Connection = PostgresConnection;
    async fn lock_accounts(
        &self,
        c: &mut Self::Connection,
        ids: &[AccountId],
    ) -> error_stack::Result<(), KernelError> {
        c.lifecycle_budget()?;
        // LOCAL only; no pool/global default changes. PG16 has no transaction_timeout:
        // the Rust transaction owner also enforces the 5s wall-clock budget including commit.
        sqlx::query("SELECT set_config('lock_timeout','2s',true), set_config('statement_timeout','3s',true), set_config('idle_in_transaction_session_timeout','5s',true)")
            .execute(&mut **c).await.convert_error()?;
        let mut ids: Vec<_> = ids.iter().map(|id| *id.as_ref()).collect();
        ids.sort_unstable();
        ids.dedup();
        for id in ids {
            sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('emumet/account-lifecycle/' || $1::text, 0))")
                .bind(id).execute(&mut **c).await.convert_error()?;
            c.note_account_gate(id);
        }
        Ok(())
    }
    async fn lock_profiles(
        &self,
        c: &mut Self::Connection,
        ids: &[ProfileId],
    ) -> error_stack::Result<(), KernelError> {
        c.lifecycle_budget()?;
        let mut ids: Vec<_> = ids.iter().map(|id| *id.as_ref()).collect();
        ids.sort_unstable();
        ids.dedup();
        for id in ids {
            if !c.acquire_profile_gate(id)? {
                continue;
            }
            sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('emumet/profile-lifecycle/' || $1::text, 0))")
                .bind(id).execute(&mut **c).await.convert_error()?;
            sqlx::query("SELECT id FROM profiles WHERE id=$1 FOR NO KEY UPDATE")
                .bind(id)
                .fetch_optional(&mut **c)
                .await
                .convert_error()?;
        }
        Ok(())
    }
    async fn lock_account_profile_set(
        &self,
        c: &mut Self::Connection,
        account: &AccountId,
        extra: &[ProfileId],
    ) -> error_stack::Result<(), KernelError> {
        let mut ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM profiles WHERE account_id=$1 UNION SELECT profile_id FROM profile_links WHERE owner_account_id=$1")
            .bind(account.as_ref()).fetch_all(&mut **c).await.convert_error()?;
        ids.extend(extra.iter().map(|id| *id.as_ref()));
        ids.sort_unstable();
        ids.dedup();
        let ids = ids.into_iter().map(ProfileId::new).collect::<Vec<_>>();
        self.lock_profiles(c, &ids).await?;
        PostgresProfileRelationshipLock
            .lock_profiles(c, &ids)
            .await?;
        Ok(())
    }
    async fn owner_candidate(
        &self,
        c: &mut Self::Connection,
        nanoid: &str,
    ) -> error_stack::Result<(ProfileId, AccountId), KernelError> {
        let (p, a): (i64, i64) =
            sqlx::query_as("SELECT id,account_id FROM profiles WHERE nanoid=$1")
                .bind(nanoid)
                .fetch_optional(&mut **c)
                .await
                .convert_error()?
                .ok_or_else(|| Report::new(KernelError::NotFound))?;
        Ok((ProfileId::new(p), AccountId::new(a)))
    }
    async fn owner(
        &self,
        c: &mut Self::Connection,
        profile: &ProfileId,
        candidate: &AccountId,
    ) -> error_stack::Result<LinkOwner, KernelError> {
        let actual: i64 = sqlx::query_scalar("SELECT account_id FROM profiles WHERE id=$1")
            .bind(profile.as_ref())
            .fetch_optional(&mut **c)
            .await
            .convert_error()?
            .ok_or_else(|| Report::new(KernelError::NotFound))?;
        if actual != *candidate.as_ref() {
            return Err(Report::new(KernelError::Concurrency).attach_printable(
                "owner changed; retry the transaction, never add an out-of-order gate",
            ));
        }
        let loaded = PostgresAccountRepository.load(c, candidate).await?;
        let account = loaded.aggregate();
        let active = account.deleted_at().is_none()
            && match account.status() {
                AccountStatus::Active => true,
                AccountStatus::Suspended {
                    expires_at: Some(end),
                    ..
                } => *end <= OffsetDateTime::now_utc(),
                _ => false,
            };
        Ok(LinkOwner {
            profile_id: profile.clone(),
            account_id: candidate.clone(),
            account_version: loaded.version().clone(),
            active,
        })
    }
    async fn account_snapshot(
        &self,
        c: &mut Self::Connection,
        account: &AccountId,
    ) -> error_stack::Result<Option<Account>, KernelError> {
        use kernel::interfaces::event_store::AccountEventStore;
        use kernel::interfaces::repository::Rehydrated;
        use kernel::prelude::entity::EventId;
        let events = super::account_event_store::PostgresAccountEventStore
            .find_by_id(c, &EventId::from(account.clone()), None)
            .await?;
        match Rehydrated::<Account>::from_events(events) {
            Ok(account) => Ok(account.map(|a| a.aggregate().clone())),
            Err(_) => {
                // Seq can commit before an earlier version. Preserve the projector's
                // bounded-window incomplete-stream retry, but never use a projection as fallback.
                tracing::warn!(account_id = %account.as_ref(), "incomplete Account stream; projection will retry");
                Ok(None)
            }
        }
    }
    async fn caller_owns(
        &self,
        c: &mut Self::Connection,
        owner: &AccountId,
        auth: &AuthAccountId,
    ) -> error_stack::Result<bool, KernelError> {
        if !self
            .account_snapshot(c, owner)
            .await?
            .is_some_and(|a| a.kind() == &kernel::prelude::entity::AccountKind::Personal)
        {
            return Ok(false);
        }
        sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM auth_emumet_accounts WHERE emumet_id=$1 AND auth_id=$2)",
        )
        .bind(owner.as_ref())
        .bind(auth.as_ref())
        .fetch_one(&mut **c)
        .await
        .convert_error()
    }
    async fn delete_owned_profiles(
        &self,
        c: &mut Self::Connection,
        account: &AccountId,
    ) -> error_stack::Result<(), KernelError> {
        c.require_account_gate(*account.as_ref())?;
        let ids: Vec<i64> =
            sqlx::query_scalar("SELECT id FROM profiles WHERE account_id=$1 ORDER BY id")
                .bind(account.as_ref())
                .fetch_all(&mut **c)
                .await
                .convert_error()?;
        let ids = ids.into_iter().map(ProfileId::new).collect::<Vec<_>>();
        self.lock_profiles(c, &ids).await?;
        // Profile deletion cascades into existing relationship tables. Take their
        // established gate before link locks / future Q1 quota or job locks.
        PostgresProfileRelationshipLock
            .lock_profiles(c, &ids)
            .await?;
        sqlx::query("DELETE FROM profiles WHERE account_id=$1")
            .bind(account.as_ref())
            .execute(&mut **c)
            .await
            .convert_error()?;
        Ok(())
    }
    async fn transfer_owner(
        &self,
        c: &mut Self::Connection,
        profile: &ProfileId,
        to: &AccountId,
    ) -> error_stack::Result<(), KernelError> {
        // Both Account gates and the Profile gate/row must already be held.
        let loaded = PostgresAccountRepository.load(c, to).await?;
        if loaded.aggregate().deleted_at().is_some() {
            return Err(Report::new(KernelError::PermissionDenied));
        }
        self.list(c, profile).await?;
        sqlx::query("UPDATE profile_links SET owner_account_id=$2, registered_account_version=$3 WHERE profile_id=$1")
            .bind(profile.as_ref()).bind(to.as_ref()).bind(loaded.version().as_ref()).execute(&mut **c).await.convert_error()?;
        Ok(())
    }
    async fn list(
        &self,
        c: &mut Self::Connection,
        profile: &ProfileId,
    ) -> error_stack::Result<Vec<HomeLink>, KernelError> {
        let rows = sqlx::query_as::<_, LinkRow>(
            "SELECT * FROM profile_links WHERE profile_id=$1 ORDER BY id FOR UPDATE",
        )
        .bind(profile.as_ref())
        .fetch_all(&mut **c)
        .await
        .convert_error()?;
        Ok(rows.into_iter().map(Into::into).collect())
    }
    async fn register(
        &self,
        c: &mut Self::Connection,
        owner: &LinkOwner,
        actor: &str,
        origin: &str,
    ) -> error_stack::Result<(HomeLink, bool), KernelError> {
        if !owner.active {
            return Err(Report::new(KernelError::PermissionDenied));
        }
        if let Some(link) = self
            .list(c, &owner.profile_id)
            .await?
            .into_iter()
            .find(|l| l.state == "active")
        {
            return if link.origin == origin {
                Ok((link, false))
            } else {
                Err(Report::new(KernelError::Concurrency)
                    .attach_printable("another home is active"))
            };
        }
        let row = sqlx::query_as::<_,LinkRow>("INSERT INTO profile_links (id,profile_id,owner_account_id,registered_account_version,actor_snapshot,origin,trust_mode,state) VALUES ($1,$2,$3,$4,$5,$6,3,'active') RETURNING *")
            .bind(kernel::prelude::entity::Nanoid::<kernel::prelude::entity::Profile>::default().as_ref().to_owned()).bind(owner.profile_id.as_ref()).bind(owner.account_id.as_ref())
            .bind(owner.account_version.as_ref()).bind(actor).bind(origin).fetch_one(&mut **c).await.convert_error()?;
        Ok((row.into(), true))
    }
    async fn unlink(
        &self,
        c: &mut Self::Connection,
        profile: &ProfileId,
        id: &str,
    ) -> error_stack::Result<(), KernelError> {
        // Lock ALL relevant link rows in ID order before Q1 can take any quota/job lock.
        let links = self.list(c, profile).await?;
        let link = links
            .into_iter()
            .find(|l| l.id == id)
            .ok_or_else(|| Report::new(KernelError::NotFound))?;
        if link.state == "unlinked" {
            return Ok(());
        }
        sqlx::query("UPDATE profile_links SET state='unlinked', generation=generation+1, unlinked_at=clock_timestamp(), unlink_reason='owner' WHERE id=$1")
            .bind(id).execute(&mut **c).await.convert_error()?;
        PostgresLinkCancellation
            .cancel_link_generation(c, id, link.generation)
            .await
    }
    async fn deactivate(
        &self,
        c: &mut Self::Connection,
        account: &AccountId,
        version: &EventVersion<Account>,
    ) -> error_stack::Result<(), KernelError> {
        // Caller owns Account gate. Include ledger-only identities and ALL owned live Profiles.
        let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM profiles WHERE account_id=$1 UNION SELECT profile_id FROM profile_links WHERE owner_account_id=$1 ORDER BY 1")
            .bind(account.as_ref()).fetch_all(&mut **c).await.convert_error()?;
        let ids = ids.into_iter().map(ProfileId::new).collect::<Vec<_>>();
        self.lock_profiles(c, &ids).await?;
        // Profile deletion cascades into existing relationship tables. Take their
        // established gate before link locks / future Q1 quota or job locks.
        PostgresProfileRelationshipLock
            .lock_profiles(c, &ids)
            .await?;
        let links = sqlx::query_as::<_,LinkRow>("SELECT * FROM profile_links WHERE owner_account_id=$1 AND registered_account_version < $2 AND state='active' ORDER BY id FOR UPDATE")
            .bind(account.as_ref()).bind(version.as_ref()).fetch_all(&mut **c).await.convert_error()?;
        for link in &links {
            sqlx::query("UPDATE profile_links SET state='unlinked', generation=generation+1, unlinked_at=clock_timestamp(), unlink_reason='account_deactivated', deactivated_account_id=$2, deactivated_account_version=$3 WHERE id=$1")
                .bind(&link.id).bind(account.as_ref()).bind(version.as_ref()).execute(&mut **c).await.convert_error()?;
        }
        for link in links {
            PostgresLinkCancellation
                .cancel_link_generation(c, &link.id, link.generation)
                .await?;
        }
        Ok(())
    }
}
impl DependOnHomeLinkStore for PostgresDatabase {
    type HomeLinkStore = PostgresHomeLinkStore;
    fn home_link_store(&self) -> &Self::HomeLinkStore {
        &PostgresHomeLinkStore
    }
}

#[cfg(test)]
mod tests;
