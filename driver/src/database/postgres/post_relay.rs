//! DB-only relay adapter. Every mutation joins the caller's transaction (no HTTP here).
mod admission;
mod delivery;
pub(super) mod maintenance;
mod operator;
#[cfg(test)]
mod tests;
use super::home_link::PostgresHomeLinkStore;
use super::profile_relationship_lock::PostgresProfileRelationshipLock;
use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use error_stack::Report;
use kernel::interfaces::home_link::{HomeLink, HomeLinkStore};
use kernel::interfaces::post_relay::*;
use kernel::interfaces::repository::ProfileRelationshipLock;
use kernel::prelude::entity::{AccountId, ProfileId};
use kernel::KernelError;
use sqlx::Row;
use time::{Duration, OffsetDateTime};

#[derive(Default)]
pub struct PostgresPostRelayStore {
    #[cfg(test)]
    now: Option<OffsetDateTime>,
}
const STORE: PostgresPostRelayStore = PostgresPostRelayStore {
    #[cfg(test)]
    now: None,
};
impl DependOnPostRelayStore for PostgresDatabase {
    type PostRelayStore = PostgresPostRelayStore;
    fn post_relay_store(&self) -> &Self::PostRelayStore {
        &STORE
    }
}
struct Gate {
    profile: i64,
    owner: i64,
    active: bool,
    actor: bool,
    links: Vec<HomeLink>,
}
#[derive(sqlx::FromRow)]
struct Quota {
    body_count: i64,
    body_bytes: i64,
    delivery_rows: i64,
    delivery_bytes: i64,
    unlinked_rows: i64,
    operator_rows: i64,
    tokens: f64,
    token_at: OffsetDateTime,
}
#[derive(sqlx::FromRow)]
struct Job {
    receipt: String,
    profile_id: i64,
    link_id: String,
    generation: i64,
    origin: String,
    logical_bytes: i64,
    automatic_expires_at: OffsetDateTime,
    token: i64,
    lease_until: Option<OffsetDateTime>,
    authorized_at: Option<OffsetDateTime>,
    send_deadline: Option<OffsetDateTime>,
    operation_id: Option<String>,
    total_attempts: i64,
    quarantine_expires_at: Option<OffsetDateTime>,
}
const JOB_SELECT:&str="SELECT b.receipt,b.profile_id,b.link_id,b.generation,b.origin,b.logical_bytes,b.automatic_expires_at,j.token,j.lease_until,j.authorized_at,j.send_deadline,j.operation_id,j.total_attempts,d.expires_at AS quarantine_expires_at FROM post_relay_bodies b JOIN post_relay_jobs j USING(receipt) LEFT JOIN post_relay_dead_letters d USING(receipt)";
impl Job {
    fn expires(&self) -> OffsetDateTime {
        if self.operation_id.is_some() {
            self.quarantine_expires_at
                .unwrap_or(self.automatic_expires_at)
        } else {
            self.automatic_expires_at
        }
    }
    fn live(&self, g: &Gate) -> bool {
        g.links
            .iter()
            .any(|l| l.id == self.link_id && l.generation == self.generation && l.state == "active")
    }
}
fn tx(c: &PostgresConnection) -> error_stack::Result<(), KernelError> {
    if c.is_transaction() {
        Ok(())
    } else {
        Err(Report::new(KernelError::Internal)
            .attach_printable("post relay mutation requires a transaction"))
    }
}
fn batch(limit: u16) -> error_stack::Result<i64, KernelError> {
    if (1..=MAX_BATCH).contains(&limit) {
        Ok(i64::from(limit))
    } else {
        Err(Report::new(KernelError::Validation))
    }
}
fn bounded(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value.bytes().all(|b| b.is_ascii_graphic() || b == b' ')
}
impl PostgresPostRelayStore {
    async fn now(
        &self,
        c: &mut PostgresConnection,
    ) -> error_stack::Result<OffsetDateTime, KernelError> {
        #[cfg(test)]
        if let Some(t) = self.now {
            return Ok(t);
        }
        sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&mut **c)
            .await
            .convert_error()
    }
    async fn candidate(
        &self,
        c: &mut PostgresConnection,
        p: i64,
    ) -> error_stack::Result<Option<i64>, KernelError> {
        sqlx::query_scalar("SELECT COALESCE((SELECT account_id FROM profiles WHERE id=$1),(SELECT owner_account_id FROM profile_links WHERE profile_id=$1 ORDER BY created_at DESC,id DESC LIMIT 1),(SELECT owner_account_id FROM post_relay_profile_quota WHERE profile_id=$1))")
   .bind(p).fetch_one(&mut **c).await.convert_error()
    }
    async fn gate(
        &self,
        c: &mut PostgresConnection,
        p: &ProfileId,
    ) -> error_stack::Result<Gate, KernelError> {
        tx(c)?;
        let id = *p.as_ref();
        let a = self
            .candidate(c, id)
            .await?
            .ok_or_else(|| Report::new(KernelError::NotFound))?;
        PostgresHomeLinkStore
            .lock_accounts(c, &[AccountId::new(a)])
            .await?;
        PostgresHomeLinkStore
            .lock_profiles(c, std::slice::from_ref(p))
            .await?;
        if self.candidate(c, id).await? != Some(a) {
            return Err(Report::new(KernelError::Concurrency));
        }
        let live: Option<bool> =
            sqlx::query_scalar("SELECT username IS NOT NULL FROM profiles WHERE id=$1")
                .bind(id)
                .fetch_optional(&mut **c)
                .await
                .convert_error()?;
        let active = if live.is_some() {
            PostgresHomeLinkStore
                .owner(c, p, &AccountId::new(a))
                .await?
                .active
        } else {
            false
        };
        // I1 joins existing receipt/Undo relationship work after prepare. Settle its
        // advisory gate BEFORE link rows here, so enqueue never climbs from link to relation.
        PostgresProfileRelationshipLock
            .lock_profiles(c, std::slice::from_ref(p))
            .await?;
        let links = PostgresHomeLinkStore.list(c, p).await?;
        Ok(Gate {
            profile: id,
            owner: a,
            active,
            actor: live == Some(true),
            links,
        })
    }
    async fn quota(
        &self,
        c: &mut PostgresConnection,
        g: &Gate,
    ) -> error_stack::Result<(Quota, Quota), KernelError> {
        let q =
            sqlx::query_as::<_, Quota>("SELECT * FROM post_relay_quota WHERE singleton FOR UPDATE")
                .fetch_one(&mut **c)
                .await
                .convert_error()?;
        let state: Option<bool> = sqlx::query_scalar(
            "SELECT delivery_state FROM post_relay_profile_quota WHERE profile_id=$1",
        )
        .bind(g.profile)
        .fetch_optional(&mut **c)
        .await
        .convert_error()?;
        let delivery = g.links.iter().any(|l| l.state == "active");
        if state.is_none() || (state == Some(false) && delivery) {
            if delivery && (q.delivery_rows >= 1_800_000 || q.delivery_bytes + 512 > 1_887_436_800)
                || !delivery && q.unlinked_rows >= 100000
            {
                return Err(Report::new(KernelError::Rejected)
                    .attach_printable("post relay global metadata capacity"));
            }
            let now = self.now(c).await?;
            if state.is_none() {
                sqlx::query("INSERT INTO post_relay_profile_quota(profile_id,owner_account_id,delivery_state,delivery_rows,delivery_bytes,unlinked_rows,token_at,touched_at) VALUES($1,$2,$3,$4,$5,$6,$7,$7)").bind(g.profile).bind(g.owner).bind(delivery).bind(i64::from(delivery)).bind(if delivery{512i64}else{0}).bind(i64::from(!delivery)).bind(now).execute(&mut **c).await.convert_error()?;
            } else {
                sqlx::query("UPDATE post_relay_profile_quota SET delivery_state=true,delivery_rows=delivery_rows+1,delivery_bytes=delivery_bytes+512,unlinked_rows=unlinked_rows-1 WHERE profile_id=$1").bind(g.profile).execute(&mut **c).await.convert_error()?;
            }
            sqlx::query("UPDATE post_relay_quota SET delivery_rows=delivery_rows+$1,delivery_bytes=delivery_bytes+$2,unlinked_rows=unlinked_rows+$3 WHERE singleton").bind(i64::from(delivery)).bind(if delivery{512i64}else{0}).bind(if state.is_some(){-1i64}else{i64::from(!delivery)}).execute(&mut **c).await.convert_error()?;
        }
        sqlx::query("UPDATE post_relay_profile_quota SET owner_account_id=$2 WHERE profile_id=$1")
            .bind(g.profile)
            .bind(g.owner)
            .execute(&mut **c)
            .await
            .convert_error()?;
        let p = sqlx::query_as::<_, Quota>(
            "SELECT * FROM post_relay_profile_quota WHERE profile_id=$1 FOR UPDATE",
        )
        .bind(g.profile)
        .fetch_one(&mut **c)
        .await
        .convert_error()?;
        sqlx::query("SELECT id FROM post_relay_slots ORDER BY id FOR UPDATE")
            .fetch_all(&mut **c)
            .await
            .convert_error()?;
        let q = sqlx::query_as::<_, Quota>("SELECT * FROM post_relay_quota WHERE singleton")
            .fetch_one(&mut **c)
            .await
            .convert_error()?;
        Ok((q, p))
    }
    async fn delta(
        &self,
        c: &mut PostgresConnection,
        p: i64,
        bodies: i64,
        bytes: i64,
        rows: i64,
        meta: i64,
    ) -> error_stack::Result<(), KernelError> {
        sqlx::query("UPDATE post_relay_quota SET body_count=body_count+$1,body_bytes=body_bytes+$2,delivery_rows=delivery_rows+$3,delivery_bytes=delivery_bytes+$4 WHERE singleton").bind(bodies).bind(bytes).bind(rows).bind(meta).execute(&mut **c).await.convert_error()?;
        let n=sqlx::query("UPDATE post_relay_profile_quota SET body_count=body_count+$2,body_bytes=body_bytes+$3,delivery_rows=delivery_rows+$4,delivery_bytes=delivery_bytes+$5,touched_at=CASE WHEN $2<>0 OR $3<>0 OR $4<>0 OR $5<>0 THEN GREATEST(touched_at,$6) ELSE touched_at END WHERE profile_id=$1").bind(p).bind(bodies).bind(bytes).bind(rows).bind(meta).bind(self.now(c).await?).execute(&mut **c).await.convert_error()?.rows_affected();
        if n != 1 {
            return Err(Report::new(KernelError::Internal)
                .attach_printable("post relay reservation mismatch"));
        }
        Ok(())
    }
    async fn usage(
        &self,
        c: &mut PostgresConnection,
        link: &str,
        bodies: i64,
        bytes: i64,
        jobs: i64,
    ) -> error_stack::Result<(), KernelError> {
        let n=sqlx::query("UPDATE post_relay_link_usage SET body_count=body_count+$2,body_bytes=body_bytes+$3,auto_jobs=auto_jobs+$4 WHERE link_id=$1 AND NOT released").bind(link).bind(bodies).bind(bytes).bind(jobs).execute(&mut **c).await.convert_error()?.rows_affected();
        if n != 1 {
            return Err(Report::new(KernelError::Internal)
                .attach_printable("post relay released or missing reservation"));
        }
        Ok(())
    }
    async fn job(
        &self,
        c: &mut PostgresConnection,
        receipt: &str,
    ) -> error_stack::Result<Option<Job>, KernelError> {
        sqlx::query_as::<_, Job>(&format!(
            "{JOB_SELECT} WHERE b.receipt=$1 FOR UPDATE OF b,j"
        ))
        .bind(receipt)
        .fetch_optional(&mut **c)
        .await
        .convert_error()
    }
    async fn leased_job(
        &self,
        c: &mut PostgresConnection,
        g: &Gate,
        l: &RelayLease,
        now: OffsetDateTime,
    ) -> error_stack::Result<Option<Job>, KernelError> {
        Ok(self.job(c, &l.receipt).await?.filter(|j| {
            j.profile_id == g.profile
                && j.token == l.token
                && j.lease_until.is_some_and(|t| t > now)
                && j.live(g)
        }))
    }
    async fn release_slot(
        &self,
        c: &mut PostgresConnection,
        token: i64,
    ) -> error_stack::Result<(), KernelError> {
        sqlx::query(
            "UPDATE post_relay_slots SET token=NULL,origin=NULL,until_at=NULL WHERE token=$1",
        )
        .bind(token)
        .execute(&mut **c)
        .await
        .convert_error()?;
        Ok(())
    }
    async fn remove_body(
        &self,
        c: &mut PostgresConnection,
        j: &Job,
    ) -> error_stack::Result<(), KernelError> {
        let auto = i64::from(j.operation_id.is_none());
        self.delta(
            c,
            j.profile_id,
            -1,
            -j.logical_bytes,
            -auto * RESERVATION_ROWS,
            -auto * RESERVATION_BYTES,
        )
        .await?;
        self.usage(c, &j.link_id, -1, -j.logical_bytes, -auto)
            .await?;
        sqlx::query("DELETE FROM post_relay_bodies WHERE receipt=$1")
            .bind(&j.receipt)
            .execute(&mut **c)
            .await
            .convert_error()?;
        sqlx::query("DELETE FROM post_relay_link_usage WHERE link_id=$1 AND body_count=0")
            .bind(&j.link_id)
            .execute(&mut **c)
            .await
            .convert_error()?;
        Ok(())
    }
    async fn quarantine(
        &self,
        c: &mut PostgresConnection,
        j: &Job,
        reason: &str,
        now: OffsetDateTime,
    ) -> error_stack::Result<(), KernelError> {
        if let Some(op) = &j.operation_id {
            sqlx::query("UPDATE post_relay_operations SET failed=failed+1 WHERE id=$1")
                .bind(op)
                .execute(&mut **c)
                .await
                .convert_error()?;
        } else {
            sqlx::query("INSERT INTO post_relay_dead_letters(receipt,terminal_at,expires_at,reason) VALUES($1,$2,$3,$4)").bind(&j.receipt).bind(now).bind(now+QUARANTINE_TTL).bind(reason).execute(&mut **c).await.convert_error()?;
            sqlx::query("INSERT INTO post_relay_audits(receipt,profile_id,link_id,reason,total_attempts,first_attempt_at,last_attempt_at,last_status,terminal_at,expires_at) SELECT b.receipt,b.profile_id,b.link_id,$2,j.total_attempts,j.first_attempt_at,j.last_attempt_at,j.last_status,$3,$4 FROM post_relay_bodies b JOIN post_relay_jobs j USING(receipt) WHERE b.receipt=$1").bind(&j.receipt).bind(reason).bind(now).bind(now+QUARANTINE_TTL).execute(&mut **c).await.convert_error()?;
            self.delta(
                c,
                j.profile_id,
                0,
                0,
                1 - RESERVATION_ROWS,
                1024 - RESERVATION_BYTES,
            )
            .await?;
            self.usage(c, &j.link_id, 0, 0, -1).await?;
            let sql = if reason == "expired" {
                "UPDATE post_relay_counters SET expired=expired+1 WHERE singleton"
            } else {
                "UPDATE post_relay_counters SET permanent=permanent+1 WHERE singleton"
            };
            sqlx::query(sql).execute(&mut **c).await.convert_error()?;
        }
        sqlx::query("DELETE FROM post_relay_jobs WHERE receipt=$1")
            .bind(&j.receipt)
            .execute(&mut **c)
            .await
            .convert_error()?;
        Ok(())
    }
}
impl PostRelayStore for PostgresPostRelayStore {
    type Connection = PostgresConnection;
    async fn admission_time(
        &self,
        c: &mut Self::Connection,
    ) -> error_stack::Result<OffsetDateTime, KernelError> {
        tx(c)?;
        self.now(c).await
    }
    async fn prepare_admission(
        &self,
        c: &mut Self::Connection,
        p: &ProfileId,
    ) -> error_stack::Result<Option<RelayRecipient>, KernelError> {
        self.prepare(c, p).await
    }
    async fn enqueue(
        &self,
        c: &mut Self::Connection,
        r: &RelayAdmission,
    ) -> error_stack::Result<Admission, KernelError> {
        self.admit(c, r).await
    }
    async fn claim_due(
        &self,
        c: &mut Self::Connection,
        limit: u16,
    ) -> error_stack::Result<Vec<RelayLease>, KernelError> {
        self.claim(c, limit).await
    }
    async fn renew_lease(
        &self,
        c: &mut Self::Connection,
        l: &RelayLease,
    ) -> error_stack::Result<Recorded, KernelError> {
        self.renew(c, l).await
    }
    async fn authorize_send(
        &self,
        c: &mut Self::Connection,
        l: &RelayLease,
        allowed: bool,
    ) -> error_stack::Result<Authorization, KernelError> {
        self.authorize(c, l, allowed).await
    }
    async fn record_attempt(
        &self,
        c: &mut Self::Connection,
        l: &RelayLease,
        r: AttemptResult,
        random: u64,
    ) -> error_stack::Result<Recorded, KernelError> {
        self.record(c, l, r, random).await
    }
    async fn hold(
        &self,
        c: &mut Self::Connection,
        l: &RelayLease,
    ) -> error_stack::Result<Recorded, KernelError> {
        self.defer(c, l).await
    }
    async fn maintenance_due(
        &self,
        c: &mut Self::Connection,
        limit: u16,
    ) -> error_stack::Result<Vec<ProfileId>, KernelError> {
        self.due(c, limit).await
    }
    async fn expire_due(
        &self,
        c: &mut Self::Connection,
        p: &ProfileId,
        limit: u16,
    ) -> error_stack::Result<u64, KernelError> {
        self.expire(c, p, limit).await
    }
    async fn purge_expired(
        &self,
        c: &mut Self::Connection,
        p: &ProfileId,
        limit: u16,
    ) -> error_stack::Result<u64, KernelError> {
        self.purge(c, p, limit).await
    }
    async fn operator_preview(
        &self,
        c: &mut Self::Connection,
        r: &OperatorRequest,
    ) -> error_stack::Result<OperatorPreview, KernelError> {
        self.preview(c, r).await
    }
    async fn begin_operator(
        &self,
        c: &mut Self::Connection,
        r: &OperatorRequest,
    ) -> error_stack::Result<OperatorPreview, KernelError> {
        self.begin(c, r).await
    }
    async fn operator_redeliver(
        &self,
        c: &mut Self::Connection,
        id: &str,
    ) -> error_stack::Result<OperatorBatch, KernelError> {
        self.operate(c, id, false).await
    }
    async fn operator_discard(
        &self,
        c: &mut Self::Connection,
        id: &str,
        confirmed: bool,
    ) -> error_stack::Result<OperatorBatch, KernelError> {
        if !confirmed {
            return Err(Report::new(KernelError::Validation));
        }
        self.operate(c, id, true).await
    }
    async fn report_capacity(
        &self,
        c: &mut Self::Connection,
        r: &CapacityReport,
    ) -> error_stack::Result<(), KernelError> {
        self.capacity_report(c, r).await
    }
    async fn read_capacity(
        &self,
        c: &mut Self::Connection,
    ) -> error_stack::Result<RelayCapacity, KernelError> {
        self.capacity(c).await
    }
    async fn check_reservations(
        &self,
        c: &mut Self::Connection,
    ) -> error_stack::Result<bool, KernelError> {
        self.consistency(c).await
    }
}
