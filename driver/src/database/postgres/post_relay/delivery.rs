use super::*;
impl PostgresPostRelayStore {
    pub(super) async fn claim(
        &self,
        c: &mut PostgresConnection,
        limit: u16,
    ) -> error_stack::Result<Vec<RelayLease>, KernelError> {
        tx(c)?;
        let limit = batch(limit)?;
        let now = self.now(c).await?;
        let rows=sqlx::query("WITH picked AS (SELECT j.receipt FROM post_relay_jobs j JOIN post_relay_bodies b USING(receipt) JOIN profile_links l ON l.id=b.link_id WHERE l.state='active' AND l.generation=b.generation AND j.due_at<=$1 AND (j.operation_id IS NULL OR j.authorized_at IS NULL) AND (j.lease_until IS NULL OR j.lease_until<=$1) AND (j.send_deadline IS NULL OR j.send_deadline<=$1) ORDER BY j.due_at,j.receipt FOR UPDATE OF j SKIP LOCKED LIMIT $2) UPDATE post_relay_jobs j SET token=nextval('post_relay_fence'),lease_until=$3,authorized_at=NULL,send_deadline=NULL,attempt_id=NULL FROM picked p,post_relay_bodies b WHERE j.receipt=p.receipt AND b.receipt=j.receipt RETURNING j.receipt,b.profile_id,j.token,j.lease_until").bind(now).bind(limit).bind(now+LEASE_TTL).fetch_all(&mut **c).await.convert_error()?;
        rows.into_iter()
            .map(|r| {
                Ok(RelayLease {
                    receipt: r.get("receipt"),
                    profile_id: ProfileId::new(r.get::<i64, _>("profile_id")),
                    token: r.get("token"),
                    lease_until: r.get("lease_until"),
                })
            })
            .collect()
    }
    pub(super) async fn preparation(
        &self,
        c: &mut PostgresConnection,
        l: &RelayLease,
    ) -> error_stack::Result<Option<RelayPreparation>, KernelError> {
        tx(c)?;
        let now = self.now(c).await?;
        // Snapshot only: never retain locks while preparing keys/signatures/SSRF policy.
        let row=sqlx::query("SELECT b.link_id,b.generation,b.origin,b.envelope,(j.operation_id IS NOT NULL) AS manual,CASE WHEN j.operation_id IS NULL THEN b.automatic_expires_at ELSE d.expires_at END AS expires_at FROM post_relay_bodies b JOIN post_relay_jobs j USING(receipt) JOIN profile_links h ON h.id=b.link_id LEFT JOIN post_relay_dead_letters d USING(receipt) WHERE b.receipt=$1 AND j.token=$2 AND b.profile_id=$3 AND j.lease_until>$4 AND j.authorized_at IS NULL AND h.state='active' AND h.generation=b.generation AND (CASE WHEN j.operation_id IS NULL THEN b.automatic_expires_at ELSE d.expires_at END)>$4").bind(&l.receipt).bind(l.token).bind(l.profile_id.as_ref()).bind(now).fetch_optional(&mut **c).await.convert_error()?;
        Ok(row.map(|r| RelayPreparation {
            recipient: RelayRecipient {
                link_id: r.get("link_id"),
                generation: r.get("generation"),
                origin: r.get("origin"),
            },
            envelope: r.get("envelope"),
            expires_at: r.get("expires_at"),
            manual: r.get("manual"),
        }))
    }
    pub(super) async fn renew(
        &self,
        c: &mut PostgresConnection,
        l: &RelayLease,
    ) -> error_stack::Result<Recorded, KernelError> {
        let g = self.gate(c, &l.profile_id).await?;
        self.quota(c, &g).await?;
        let now = self.now(c).await?;
        let Some(j) = self.leased_job(c, &g, l, now).await? else {
            return Ok(Recorded::Stale);
        };
        if j.expires() <= now || j.send_deadline.is_some_and(|d| d <= now) {
            return Ok(Recorded::Stale);
        }
        sqlx::query("UPDATE post_relay_jobs SET lease_until=$2 WHERE receipt=$1")
            .bind(&l.receipt)
            .bind(now + LEASE_TTL)
            .execute(&mut **c)
            .await
            .convert_error()?;
        Ok(Recorded::Applied)
    }
    pub(super) async fn authorize(
        &self,
        c: &mut PostgresConnection,
        l: &RelayLease,
        allowed: bool,
    ) -> error_stack::Result<Authorization, KernelError> {
        let g = self.gate(c, &l.profile_id).await?;
        self.quota(c, &g).await?;
        let now = self.now(c).await?;
        let Some(j) = self.leased_job(c, &g, l, now).await? else {
            return Ok(Authorization::Stale);
        };
        if j.authorized_at.is_some() {
            return Ok(Authorization::Stale);
        }
        if now >= j.expires() {
            self.quarantine(c, &j, "expired", now).await?;
            return Ok(Authorization::Expired);
        }
        if !g.active || !g.actor || !allowed {
            self.hold_job(c, &j, now).await?;
            return Ok(Authorization::Held);
        }
        let in_origin: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM post_relay_slots WHERE origin=$1 AND until_at>$2",
        )
        .bind(&j.origin)
        .bind(now)
        .fetch_one(&mut **c)
        .await
        .convert_error()?;
        let slot:Option<i16>=sqlx::query_scalar("SELECT id FROM post_relay_slots WHERE until_at IS NULL OR until_at<=$1 ORDER BY id LIMIT 1").bind(now).fetch_optional(&mut **c).await.convert_error()?;
        let Some(slot) = slot.filter(|_| in_origin < 4) else {
            return Ok(Authorization::Busy);
        };
        let envelope: Vec<u8> =
            sqlx::query_scalar("SELECT envelope FROM post_relay_bodies WHERE receipt=$1")
                .bind(&j.receipt)
                .fetch_one(&mut **c)
                .await
                .convert_error()?;
        // The permission UPDATE samples DB time, once. Commit latency consumes the same deadline.
        #[cfg(not(test))]
        let injected: Option<OffsetDateTime> = None;
        #[cfg(test)]
        let injected = self.now;
        let updated=sqlx::query("WITH clock AS MATERIALIZED (SELECT COALESCE($3::timestamptz,clock_timestamp()) AS now) UPDATE post_relay_jobs j SET authorized_at=clock.now,send_deadline=LEAST(clock.now+interval '10 seconds',$4),attempt_id=nextval('post_relay_fence'),lease_until=clock.now+interval '30 seconds' FROM clock WHERE j.receipt=$1 AND j.token=$2 AND j.lease_until>clock.now AND $4::timestamptz>clock.now RETURNING j.authorized_at,j.send_deadline,j.attempt_id").bind(&j.receipt).bind(j.token).bind(injected).bind(j.expires()).fetch_optional(&mut **c).await.convert_error()?;
        let Some(updated) = updated else {
            let now = self.now(c).await?;
            if now >= j.expires() {
                self.quarantine(c, &j, "expired", now).await?;
                return Ok(Authorization::Expired);
            }
            return Ok(Authorization::Stale);
        };
        let authorized_at: OffsetDateTime = updated.get("authorized_at");
        let deadline: OffsetDateTime = updated.get("send_deadline");
        let attempt_id: i64 = updated.get("attempt_id");
        sqlx::query("UPDATE post_relay_slots SET origin=$2,token=$3,until_at=$4 WHERE id=$1")
            .bind(slot)
            .bind(&j.origin)
            .bind(j.token)
            .bind(deadline)
            .execute(&mut **c)
            .await
            .convert_error()?;
        Ok(Authorization::Permitted(SendPermit {
            lease: RelayLease {
                receipt: j.receipt,
                profile_id: l.profile_id.clone(),
                token: j.token,
                lease_until: authorized_at + LEASE_TTL,
            },
            recipient: RelayRecipient {
                link_id: j.link_id,
                generation: j.generation,
                origin: j.origin,
            },
            envelope,
            attempt_id,
            authorized_at,
            send_deadline: deadline,
            manual: j.operation_id.is_some(),
        }))
    }
    async fn hold_job(
        &self,
        c: &mut PostgresConnection,
        j: &Job,
        now: OffsetDateTime,
    ) -> error_stack::Result<(), KernelError> {
        sqlx::query("UPDATE post_relay_jobs SET due_at=$2,lease_until=NULL WHERE receipt=$1")
            .bind(&j.receipt)
            .bind((now + Duration::seconds(30)).min(j.expires()))
            .execute(&mut **c)
            .await
            .convert_error()?;
        sqlx::query("UPDATE post_relay_counters SET held=held+1 WHERE singleton")
            .execute(&mut **c)
            .await
            .convert_error()?;
        Ok(())
    }
    pub(super) async fn defer(
        &self,
        c: &mut PostgresConnection,
        l: &RelayLease,
    ) -> error_stack::Result<Recorded, KernelError> {
        let g = self.gate(c, &l.profile_id).await?;
        self.quota(c, &g).await?;
        let now = self.now(c).await?;
        let Some(j) = self.leased_job(c, &g, l, now).await? else {
            return Ok(Recorded::Stale);
        };
        if j.authorized_at.is_some() {
            return Ok(Recorded::Stale);
        }
        if now >= j.expires() {
            self.quarantine(c, &j, "expired", now).await?;
        } else {
            self.hold_job(c, &j, now).await?;
        }
        Ok(Recorded::Applied)
    }
    pub(super) async fn record(
        &self,
        c: &mut PostgresConnection,
        l: &RelayLease,
        result: AttemptResult,
        observation: Option<&AttemptObservation>,
        random: u64,
    ) -> error_stack::Result<Recorded, KernelError> {
        let g = self.gate(c, &l.profile_id).await?;
        self.quota(c, &g).await?;
        let now = self.now(c).await?;
        let Some(j) = self.leased_job(c, &g, l, now).await? else {
            return Ok(Recorded::Stale);
        };
        let observed_at = if let Some(o) = observation {
            if j.attempt_id != Some(o.attempt_id) {
                return Ok(Recorded::Stale);
            }
            let (Some(authorized), Some(deadline)) = (j.authorized_at, j.send_deadline) else {
                return Ok(Recorded::Stale);
            };
            if o.observed_at < authorized || o.observed_at > deadline {
                return Err(Report::new(KernelError::Validation));
            }
            o.observed_at
        } else {
            // No completed HTTP response: pre-permission preparation failure or transport timeout.
            // A definite response after permission requires the original attempt and observation.
            if result == AttemptResult::Success
                || (j.authorized_at.is_some()
                    && !matches!(result, AttemptResult::Retry { status: None, .. }))
            {
                return Err(Report::new(KernelError::Validation));
            }
            now
        };
        let status = match result {
            AttemptResult::Success => Some(200),
            AttemptResult::Retry { status, .. } | AttemptResult::Permanent { status } => status,
        };
        if status.is_some_and(|s| !(100..=599).contains(&s)) {
            return Err(Report::new(KernelError::Validation));
        }
        self.release_slot(c, j.token).await?;
        let completed_terminal = observation.is_some()
            && matches!(
                result,
                AttemptResult::Success | AttemptResult::Permanent { .. }
            );
        let expired = now >= j.expires() && !completed_terminal;
        let reason = if expired {
            "expired"
        } else {
            match result {
                AttemptResult::Success => "success",
                AttemptResult::Retry { .. } => "retry",
                AttemptResult::Permanent { .. } => "permanent",
            }
        };
        sqlx::query("UPDATE post_relay_jobs SET total_attempts=total_attempts+1,first_attempt_at=COALESCE(first_attempt_at,$2),last_attempt_at=$2,last_status=$3 WHERE receipt=$1").bind(&j.receipt).bind(observed_at).bind(status.map(|s|s as i16)).execute(&mut **c).await.convert_error()?;
        if j.operation_id.is_none() {
            sqlx::query("INSERT INTO post_relay_attempts(receipt,number,at,status,reason) VALUES($1,$2,$3,$4,$5)").bind(&j.receipt).bind(j.total_attempts+1).bind(observed_at).bind(status.map(|s|s as i16)).bind(reason).execute(&mut **c).await.convert_error()?;
            sqlx::query("DELETE FROM post_relay_attempts WHERE receipt=$1 AND number<=$2")
                .bind(&j.receipt)
                .bind(j.total_attempts + 1 - ATTEMPT_ROWS)
                .execute(&mut **c)
                .await
                .convert_error()?;
        }
        if expired {
            self.quarantine(c, &j, "expired", now).await?;
        } else if result == AttemptResult::Success {
            if let Some(op) = &j.operation_id {
                sqlx::query("UPDATE post_relay_operations SET succeeded=succeeded+1 WHERE id=$1")
                    .bind(op)
                    .execute(&mut **c)
                    .await
                    .convert_error()?;
            }
            self.remove_body(c, &j).await?;
            sqlx::query("UPDATE post_relay_counters SET succeeded=succeeded+1 WHERE singleton")
                .execute(&mut **c)
                .await
                .convert_error()?;
        } else if matches!(result, AttemptResult::Permanent { .. }) || j.operation_id.is_some() {
            self.quarantine_observed(c, &j, "permanent", now, observed_at)
                .await?;
        } else if let AttemptResult::Retry {
            retry_after_secs, ..
        } = result
        {
            let delay = retry_delay(
                (j.total_attempts + 1) as u64,
                random,
                retry_after_secs,
                j.expires() - now,
            );
            sqlx::query("UPDATE post_relay_jobs SET due_at=$2,lease_until=NULL,authorized_at=NULL,send_deadline=NULL,attempt_id=NULL WHERE receipt=$1").bind(&j.receipt).bind(now+delay).execute(&mut **c).await.convert_error()?;
            sqlx::query("UPDATE post_relay_counters SET retries=retries+1 WHERE singleton")
                .execute(&mut **c)
                .await
                .convert_error()?;
        }
        Ok(Recorded::Applied)
    }
}
