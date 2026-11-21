use super::*;
impl PostgresPostRelayStore {
    pub(super) async fn prepare(
        &self,
        c: &mut PostgresConnection,
        p: &ProfileId,
    ) -> error_stack::Result<Option<RelayRecipient>, KernelError> {
        let g = self.gate(c, p).await?;
        if !g.active || !g.actor {
            return Err(Report::new(KernelError::NotFound));
        }
        Ok(g.links
            .iter()
            .find(|l| l.state == "active" && l.trust_mode == 3)
            .map(|l| RelayRecipient {
                link_id: l.id.clone(),
                generation: l.generation,
                origin: l.origin.clone(),
            }))
    }
    pub(super) async fn admit(
        &self,
        c: &mut PostgresConnection,
        r: &RelayAdmission,
    ) -> error_stack::Result<Admission, KernelError> {
        if !bounded(&r.receipt, 128) || r.envelope.len() > 2 * 1024 * 1024 || r.envelope.is_empty()
        {
            return Err(Report::new(KernelError::Validation));
        }
        let g = self.gate(c, &r.profile_id).await?;
        if !g.active || !g.actor {
            return Err(Report::new(KernelError::NotFound));
        }
        let current = g
            .links
            .iter()
            .find(|l| l.state == "active" && l.trust_mode == 3)
            .map(|l| RelayRecipient {
                link_id: l.id.clone(),
                generation: l.generation,
                origin: l.origin.clone(),
            });
        if current != r.recipient {
            return Err(Report::new(KernelError::Concurrency));
        }
        let (q, p) = match self.admission_quota(c, &g).await {
            Ok(value) => value,
            Err(e) if matches!(e.current_context(), KernelError::Rejected) => {
                return Ok(Admission::Limited {
                    scope: LimitScope::Global,
                    retry_after_secs: 1,
                })
            }
            Err(e) => return Err(e),
        };
        let now = self.now(c).await?;
        if r.received_at > now {
            return Err(Report::new(KernelError::Validation));
        }
        let available:bool=sqlx::query_scalar("SELECT q.consistent AND c.accepting AND c.sampled_at>=$1-interval '90 seconds' AND c.sampled_at<=$1 FROM post_relay_quota q CROSS JOIN post_relay_capacity c").bind(now).fetch_one(&mut **c).await.convert_error()?;
        if !available {
            return Ok(Admission::Unavailable);
        }
        let global = |scope| Admission::Limited {
            scope,
            retry_after_secs: 1,
        };
        let Some(recipient) = current else {
            if let Some(limited) = self.rate(c, &g, &q, &p, true, now).await? {
                return Ok(limited);
            }
            let bucket = now
                .replace_minute(0)
                .unwrap()
                .replace_second(0)
                .unwrap()
                .replace_nanosecond(0)
                .unwrap();
            let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM post_relay_unlinked WHERE profile_id=$1 AND reason='no_home' AND bucket=$2)").bind(g.profile).bind(bucket).fetch_one(&mut **c).await.convert_error()?;
            if !exists {
                if p.unlinked_rows >= 1000 {
                    return Ok(global(LimitScope::Profile));
                }
                if q.unlinked_rows >= 100000 {
                    return Ok(global(LimitScope::Global));
                }
                sqlx::query(
                    "UPDATE post_relay_quota SET unlinked_rows=unlinked_rows+1 WHERE singleton",
                )
                .execute(&mut **c)
                .await
                .convert_error()?;
                sqlx::query("UPDATE post_relay_profile_quota SET unlinked_rows=unlinked_rows+1 WHERE profile_id=$1").bind(g.profile).execute(&mut **c).await.convert_error()?;
            }
            sqlx::query("INSERT INTO post_relay_unlinked(profile_id,reason,bucket,count,first_at,last_at,expires_at) VALUES($1,'no_home',$2,1,$3,$3,$4) ON CONFLICT(profile_id,reason,bucket) DO UPDATE SET count=post_relay_unlinked.count+1,last_at=EXCLUDED.last_at").bind(g.profile).bind(bucket).bind(now).bind(now+QUARANTINE_TTL).execute(&mut **c).await.convert_error()?;
            return Ok(Admission::Unlinked);
        };
        let bytes = r.envelope.len() as i64 + BODY_METADATA_BYTES;
        if p.body_count >= 2000
            || p.body_bytes + bytes > 536_870_912
            || p.delivery_rows + RESERVATION_ROWS > 48000
            || p.delivery_bytes + RESERVATION_BYTES > 62_914_560
        {
            return Ok(global(LimitScope::Profile));
        }
        if q.body_count >= 100000
            || q.body_bytes + bytes > 21_474_836_480
            || q.delivery_rows + RESERVATION_ROWS > 1_800_000
            || q.delivery_bytes + RESERVATION_BYTES > 1_887_436_800
        {
            return Ok(global(LimitScope::Global));
        }
        if let Some(limited) = self.rate(c, &g, &q, &p, false, now).await? {
            return Ok(limited);
        }
        sqlx::query("INSERT INTO post_relay_link_usage(link_id,profile_id) VALUES($1,$2) ON CONFLICT DO NOTHING").bind(&recipient.link_id).bind(g.profile).execute(&mut **c).await.convert_error()?;
        self.usage(c, &recipient.link_id, 1, bytes, 1).await?;
        self.delta(c, g.profile, 1, bytes, RESERVATION_ROWS, RESERVATION_BYTES)
            .await?;
        sqlx::query("INSERT INTO post_relay_bodies(receipt,profile_id,link_id,generation,origin,received_at,automatic_expires_at,envelope,logical_bytes) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)").bind(&r.receipt).bind(g.profile).bind(&recipient.link_id).bind(recipient.generation).bind(&recipient.origin).bind(r.received_at).bind(r.received_at+AUTOMATIC_TTL).bind(&r.envelope).bind(bytes).execute(&mut **c).await.convert_error()?;
        sqlx::query("INSERT INTO post_relay_jobs(receipt,due_at) VALUES($1,$2)")
            .bind(&r.receipt)
            .bind(now)
            .execute(&mut **c)
            .await
            .convert_error()?;
        Ok(Admission::Queued)
    }
    async fn rate(
        &self,
        c: &mut PostgresConnection,
        g: &Gate,
        q: &Quota,
        p: &Quota,
        unlinked: bool,
        now: OffsetDateTime,
    ) -> error_stack::Result<Option<Admission>, KernelError> {
        let (tokens, at) = if unlinked {
            sqlx::query_as::<_, (f64, OffsetDateTime)>(
                "SELECT unlinked_tokens,unlinked_token_at FROM post_relay_quota WHERE singleton",
            )
            .fetch_one(&mut **c)
            .await
            .convert_error()?
        } else {
            (q.tokens, q.token_at)
        };
        let gt = (tokens + ((now - at).as_seconds_f64().max(0.0) * 1000.0)).min(1000.0);
        let pt = (p.tokens + ((now - p.token_at).as_seconds_f64().max(0.0) * 200.0)).min(200.0);
        if pt < 1.0 {
            return Ok(Some(Admission::Limited {
                scope: LimitScope::Profile,
                retry_after_secs: 1,
            }));
        }
        if gt < 1.0 {
            return Ok(Some(Admission::Limited {
                scope: LimitScope::Global,
                retry_after_secs: 1,
            }));
        }
        let sql = if unlinked {
            "UPDATE post_relay_quota SET unlinked_tokens=$1,unlinked_token_at=$2 WHERE singleton"
        } else {
            "UPDATE post_relay_quota SET tokens=$1,token_at=$2 WHERE singleton"
        };
        sqlx::query(sql)
            .bind(gt - 1.0)
            .bind(now.max(at))
            .execute(&mut **c)
            .await
            .convert_error()?;
        sqlx::query("UPDATE post_relay_profile_quota SET tokens=$2,token_at=$3,touched_at=$3 WHERE profile_id=$1").bind(g.profile).bind(pt-1.0).bind(now.max(p.token_at)).execute(&mut **c).await.convert_error()?;
        Ok(None)
    }
}
