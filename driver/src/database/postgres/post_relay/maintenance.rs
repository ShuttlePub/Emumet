use super::*;
impl PostgresPostRelayStore {
    pub(super) async fn due(
        &self,
        c: &mut PostgresConnection,
        limit: u16,
    ) -> error_stack::Result<Vec<ProfileId>, KernelError> {
        let limit = batch(limit)?;
        let now = self.now(c).await?;
        let ids:Vec<i64>=sqlx::query_scalar("SELECT profile_id FROM (SELECT b.profile_id FROM post_relay_bodies b JOIN profile_links l ON l.id=b.link_id LEFT JOIN post_relay_dead_letters d USING(receipt) LEFT JOIN post_relay_jobs j USING(receipt) WHERE l.state<>'active' OR l.generation<>b.generation OR (d.expires_at<=$1 AND (j.authorized_at IS NULL OR j.lease_until<=$1)) UNION SELECT b.profile_id FROM post_relay_bodies b JOIN post_relay_jobs j USING(receipt) WHERE (j.operation_id IS NULL AND b.automatic_expires_at<=$1 AND (j.authorized_at IS NULL OR j.lease_until<=$1)) OR (j.operation_id IS NOT NULL AND j.authorized_at IS NOT NULL AND j.lease_until<=$1 AND j.send_deadline<=$1) UNION SELECT profile_id FROM post_relay_audits WHERE expires_at<=$1 UNION SELECT profile_id FROM post_relay_unlinked WHERE expires_at<=$1 UNION SELECT profile_id FROM post_relay_operations WHERE expires_at<=$1 UNION SELECT profile_id FROM post_relay_profile_quota WHERE body_count=0 AND touched_at<=$1-interval '720 hours') candidates ORDER BY profile_id LIMIT $2").bind(now).bind(limit).fetch_all(&mut **c).await.convert_error()?;
        Ok(ids.into_iter().map(ProfileId::new).collect())
    }
    pub(super) async fn expire(
        &self,
        c: &mut PostgresConnection,
        p: &ProfileId,
        limit: u16,
    ) -> error_stack::Result<u64, KernelError> {
        let limit = batch(limit)?;
        let g = self.gate(c, p).await?;
        if !self.existing_quota(c, &g).await? {
            return Ok(0);
        }
        let now = self.now(c).await?;
        let receipts:Vec<String>=sqlx::query_scalar("SELECT b.receipt FROM post_relay_bodies b JOIN post_relay_jobs j USING(receipt) JOIN profile_links l ON l.id=b.link_id LEFT JOIN post_relay_dead_letters d USING(receipt) WHERE b.profile_id=$1 AND l.state='active' AND l.generation=b.generation AND ((j.operation_id IS NULL AND b.automatic_expires_at<=$2) OR (j.operation_id IS NOT NULL AND (d.expires_at<=$2 OR (j.authorized_at IS NOT NULL AND j.lease_until<=$2 AND j.send_deadline<=$2)))) AND (j.authorized_at IS NULL OR j.lease_until<=$2) ORDER BY b.seq LIMIT $3").bind(g.profile).bind(now).bind(limit).fetch_all(&mut **c).await.convert_error()?;
        for receipt in &receipts {
            if let Some(j) = self.job(c, receipt).await? {
                self.release_slot(c, j.token).await?;
                self.quarantine(c, &j, "expired", now).await?;
            }
        }
        Ok(receipts.len() as u64)
    }
    pub(super) async fn purge(
        &self,
        c: &mut PostgresConnection,
        p: &ProfileId,
        limit: u16,
    ) -> error_stack::Result<u64, KernelError> {
        let limit = batch(limit)?;
        let g = self.gate(c, p).await?;
        if !self.existing_quota(c, &g).await? {
            return Ok(0);
        }
        let now = self.now(c).await?;
        let bodies=sqlx::query("SELECT b.receipt,b.link_id,b.logical_bytes,l.state,l.generation=b.generation AS same_generation,u.released FROM post_relay_bodies b JOIN profile_links l ON l.id=b.link_id JOIN post_relay_link_usage u ON u.link_id=b.link_id LEFT JOIN post_relay_dead_letters d USING(receipt) LEFT JOIN post_relay_jobs j USING(receipt) WHERE b.profile_id=$1 AND (l.state<>'active' OR l.generation<>b.generation OR (d.expires_at<=$2 AND (j.authorized_at IS NULL OR j.lease_until<=$2))) ORDER BY b.seq FOR UPDATE OF b LIMIT $3").bind(g.profile).bind(now).bind(limit).fetch_all(&mut **c).await.convert_error()?;
        for b in &bodies {
            let receipt: String = b.get("receipt");
            let link: String = b.get("link_id");
            let cancelled =
                b.get::<String, _>("state") != "active" || !b.get::<bool, _>("same_generation");
            if cancelled {
                if !b.get::<bool, _>("released") {
                    return Err(Report::new(KernelError::Internal)
                        .attach_printable("post relay cancellation reservation mismatch"));
                }
            } else {
                self.delta(c, g.profile, -1, -b.get::<i64, _>("logical_bytes"), 0, 0)
                    .await?;
                self.usage(c, &link, -1, -b.get::<i64, _>("logical_bytes"), 0)
                    .await?;
            }
            if let Some(j) = self.job(c, &receipt).await? {
                if let Some(op) = j.operation_id {
                    sqlx::query(
                        "UPDATE post_relay_operations SET cancelled=cancelled+1 WHERE id=$1",
                    )
                    .bind(op)
                    .execute(&mut **c)
                    .await
                    .convert_error()?;
                }
            }
            sqlx::query("DELETE FROM post_relay_bodies WHERE receipt=$1")
                .bind(receipt)
                .execute(&mut **c)
                .await
                .convert_error()?;
        }
        let a=sqlx::query("DELETE FROM post_relay_audits WHERE receipt IN (SELECT receipt FROM post_relay_audits WHERE profile_id=$1 AND expires_at<=$2 ORDER BY expires_at,receipt LIMIT $3)").bind(g.profile).bind(now).bind(limit).execute(&mut **c).await.convert_error()?.rows_affected() as i64;
        self.delta(c, g.profile, 0, 0, -a, -a * 1024).await?;
        let u=sqlx::query("DELETE FROM post_relay_unlinked WHERE (profile_id,reason,bucket) IN (SELECT profile_id,reason,bucket FROM post_relay_unlinked WHERE profile_id=$1 AND expires_at<=$2 ORDER BY bucket LIMIT $3)").bind(g.profile).bind(now).bind(limit).execute(&mut **c).await.convert_error()?.rows_affected() as i64;
        let o=sqlx::query("DELETE FROM post_relay_operations WHERE id IN (SELECT o.id FROM post_relay_operations o WHERE o.profile_id=$1 AND o.expires_at<=$2 AND NOT EXISTS(SELECT 1 FROM post_relay_jobs j WHERE j.operation_id=o.id) ORDER BY o.expires_at,o.id LIMIT $3)").bind(g.profile).bind(now).bind(limit).execute(&mut **c).await.convert_error()?.rows_affected() as i64;
        sqlx::query("UPDATE post_relay_quota SET unlinked_rows=unlinked_rows-$1,operator_rows=operator_rows-$2 WHERE singleton").bind(u).bind(o).execute(&mut **c).await.convert_error()?;
        sqlx::query("UPDATE post_relay_profile_quota SET unlinked_rows=unlinked_rows-$2,operator_rows=operator_rows-$3 WHERE profile_id=$1").bind(g.profile).bind(u).bind(o).execute(&mut **c).await.convert_error()?;
        sqlx::query("DELETE FROM post_relay_link_usage u WHERE u.profile_id=$1 AND u.body_count=0 AND NOT EXISTS(SELECT 1 FROM post_relay_bodies b WHERE b.link_id=u.link_id)").bind(g.profile).execute(&mut **c).await.convert_error()?;
        // Empty per-Profile state is bounded too; rate history is kept 30 days, never per receipt.
        let removed:Option<bool>=sqlx::query_scalar("DELETE FROM post_relay_profile_quota WHERE profile_id=$1 AND body_count=0 AND delivery_rows=(delivery_state::int) AND unlinked_rows=((NOT delivery_state)::int) AND operator_rows=0 AND touched_at<=$2-interval '720 hours' AND NOT EXISTS(SELECT 1 FROM post_relay_bodies WHERE profile_id=$1) RETURNING delivery_state").bind(g.profile).bind(now).fetch_optional(&mut **c).await.convert_error()?;
        if let Some(delivery) = removed {
            sqlx::query("UPDATE post_relay_quota SET delivery_rows=delivery_rows-$1,delivery_bytes=delivery_bytes-$2,unlinked_rows=unlinked_rows-$3 WHERE singleton").bind(i64::from(delivery)).bind(if delivery{512i64}else{0}).bind(i64::from(!delivery)).execute(&mut **c).await.convert_error()?;
        }
        let total = bodies.len() as u64 + a as u64 + u as u64 + o as u64;
        sqlx::query(
            "UPDATE post_relay_counters SET purged=purged+$1,last_purge_at=$2 WHERE singleton",
        )
        .bind(total as i64)
        .bind(now)
        .execute(&mut **c)
        .await
        .convert_error()?;
        Ok(total)
    }
    pub(super) async fn capacity_report(
        &self,
        c: &mut PostgresConnection,
        r: &CapacityReport,
    ) -> error_stack::Result<(), KernelError> {
        tx(c)?;
        let now = self.now(c).await?;
        if r.free_bytes < 0
            || r.total_bytes <= 0
            || r.free_bytes > r.total_bytes
            || r.sampled_at > now
            || r.sampled_at < now - Duration::seconds(90)
        {
            return Err(Report::new(KernelError::Validation));
        }
        let row = sqlx::query(
            "SELECT accepting,sampled_at FROM post_relay_capacity WHERE singleton FOR UPDATE",
        )
        .fetch_one(&mut **c)
        .await
        .convert_error()?;
        let previous: Option<OffsetDateTime> = row.get("sampled_at");
        if previous.is_some_and(|p| r.sampled_at <= p) {
            return Err(Report::new(KernelError::Concurrency));
        }
        let was_accepting = row.get::<bool, _>("accepting")
            && previous.is_some_and(|p| p >= now - Duration::seconds(90));
        let threshold = if was_accepting { 10i128 } else { 12i128 };
        let accepting = i128::from(r.free_bytes) >= threshold * 1024 * 1024 * 1024
            && i128::from(r.free_bytes) * 100 >= i128::from(r.total_bytes) * threshold;
        sqlx::query("UPDATE post_relay_capacity SET free_bytes=$1,total_bytes=$2,sampled_at=$3,accepting=$4 WHERE singleton").bind(r.free_bytes).bind(r.total_bytes).bind(r.sampled_at).bind(accepting).execute(&mut **c).await.convert_error()?;
        Ok(())
    }
    pub(super) async fn capacity(
        &self,
        c: &mut PostgresConnection,
    ) -> error_stack::Result<RelayCapacity, KernelError> {
        let now = self.now(c).await?;
        let r=sqlx::query("SELECT q.*,k.succeeded,k.cancelled,k.permanent,k.expired,k.retries,k.held,k.purged,c.sampled_at,(q.consistent AND c.accepting AND c.sampled_at>=$1-interval '90 seconds' AND c.sampled_at<=$1) IS TRUE AS physical_accepting,k.last_purge_at,(SELECT min(received_at) FROM post_relay_bodies b JOIN profile_links l ON b.link_id=l.id WHERE l.state='active' AND l.generation=b.generation) AS oldest_received_at,(SELECT min(automatic_expires_at) FROM post_relay_bodies b JOIN post_relay_jobs j USING(receipt) WHERE operation_id IS NULL) AS earliest_automatic_expiry,(SELECT min(expires_at) FROM post_relay_dead_letters) AS earliest_quarantine_expiry,(SELECT count(*) FROM post_relay_bodies b JOIN profile_links l ON b.link_id=l.id WHERE l.state<>'active' OR l.generation<>b.generation) AS cancelled_body_backlog FROM post_relay_quota q CROSS JOIN post_relay_capacity c CROSS JOIN post_relay_counters k").bind(now).fetch_one(&mut **c).await.convert_error()?;
        Ok(RelayCapacity {
            succeeded: r.get("succeeded"),
            cancelled: r.get("cancelled"),
            permanent: r.get("permanent"),
            expired: r.get("expired"),
            retries: r.get("retries"),
            held: r.get("held"),
            purged: r.get("purged"),
            body_count: r.get("body_count"),
            body_bytes: r.get("body_bytes"),
            delivery_rows: r.get("delivery_rows"),
            delivery_bytes: r.get("delivery_bytes"),
            unlinked_rows: r.get("unlinked_rows"),
            operator_rows: r.get("operator_rows"),
            consistent: r.get("consistent"),
            physical_accepting: r.get("physical_accepting"),
            sampled_at: r.get("sampled_at"),
            oldest_received_at: r.get("oldest_received_at"),
            earliest_automatic_expiry: r.get("earliest_automatic_expiry"),
            earliest_quarantine_expiry: r.get("earliest_quarantine_expiry"),
            last_purge_at: r.get("last_purge_at"),
            cancelled_body_backlog: r.get("cancelled_body_backlog"),
        })
    }
    pub(super) async fn consistency(
        &self,
        c: &mut PostgresConnection,
    ) -> error_stack::Result<bool, KernelError> {
        tx(c)?;
        sqlx::query("SELECT singleton FROM post_relay_quota WHERE singleton FOR UPDATE")
            .execute(&mut **c)
            .await
            .convert_error()?;
        let ok: bool = sqlx::query_scalar(include_str!("consistency.sql"))
            .fetch_one(&mut **c)
            .await
            .convert_error()?;
        sqlx::query("UPDATE post_relay_quota SET consistent=$1 WHERE singleton")
            .bind(ok)
            .execute(&mut **c)
            .await
            .convert_error()?;
        if !ok {
            tracing::error!("post relay reservation inconsistency; new admission stopped");
        }
        Ok(ok)
    }
}
/// L1 invokes this only after all affected link rows are locked. Logical release is O(links),
/// not O(bodies/TOAST/attempts). Physical removal is a separately bounded purge transaction.
pub(in crate::database::postgres) async fn cancel(
    c: &mut PostgresConnection,
    link: &str,
    generation: i64,
) -> error_stack::Result<(), KernelError> {
    tx(c)?;
    let row = sqlx::query(
        "SELECT profile_id,owner_account_id,state,generation FROM profile_links WHERE id=$1",
    )
    .bind(link)
    .fetch_one(&mut **c)
    .await
    .convert_error()?;
    let p: i64 = row.get("profile_id");
    let a: i64 = row.get("owner_account_id");
    c.require_account_gate(a)?;
    c.require_profile_gate(p)?;
    if row.get::<String, _>("state") != "unlinked"
        || row.get::<i64, _>("generation") != generation + 1
    {
        return Err(Report::new(KernelError::Concurrency));
    }
    let u=sqlx::query("SELECT body_count,body_bytes,auto_jobs,released FROM post_relay_link_usage WHERE link_id=$1").bind(link).fetch_optional(&mut **c).await.convert_error()?;
    let Some(u) = u.filter(|u| !u.get::<bool, _>("released")) else {
        return Ok(());
    };
    if !STORE
        .existing_quota(
            c,
            &Gate {
                profile: p,
                owner: a,
                active: false,
                actor: false,
                links: vec![],
            },
        )
        .await?
    {
        return Err(Report::new(KernelError::Internal)
            .attach_printable("post relay missing cancellation quota"));
    }
    let count: i64 = u.get("body_count");
    let jobs: i64 = u.get("auto_jobs");
    STORE
        .delta(
            c,
            p,
            -count,
            -u.get::<i64, _>("body_bytes"),
            -jobs * RESERVATION_ROWS,
            -jobs * RESERVATION_BYTES,
        )
        .await?;
    sqlx::query("UPDATE post_relay_link_usage SET released=true,body_count=0,body_bytes=0,auto_jobs=0 WHERE link_id=$1").bind(link).execute(&mut **c).await.convert_error()?;
    sqlx::query("UPDATE post_relay_counters SET cancelled=cancelled+$1 WHERE singleton")
        .bind(count)
        .execute(&mut **c)
        .await
        .convert_error()?;
    Ok(())
}
