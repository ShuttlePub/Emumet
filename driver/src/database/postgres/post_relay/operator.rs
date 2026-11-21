use super::*;
impl PostgresPostRelayStore {
    fn validate_operation(r: &OperatorRequest) -> error_stack::Result<(), KernelError> {
        if !bounded(&r.id, 64)
            || !bounded(&r.operator, 128)
            || !bounded(&r.reason, 128)
            || r.receipt.as_ref().is_some_and(|s| !bounded(s, 128))
        {
            return Err(Report::new(KernelError::Validation));
        }
        Ok(())
    }
    fn operation_link(
        g: &Gate,
        action: OperatorAction,
    ) -> error_stack::Result<&HomeLink, KernelError> {
        if action == OperatorAction::Redeliver && (!g.active || !g.actor) {
            return Err(Report::new(KernelError::PermissionDenied));
        }
        g.links
            .iter()
            .find(|l| l.state == "active" && l.trust_mode == 3)
            .ok_or_else(|| Report::new(KernelError::PermissionDenied))
    }
    async fn selection(
        &self,
        c: &mut PostgresConnection,
        r: &OperatorRequest,
        link: &HomeLink,
        now: OffsetDateTime,
    ) -> error_stack::Result<OperatorPreview, KernelError> {
        let row=sqlx::query("SELECT count(*) AS n,coalesce(sum(b.logical_bytes),0)::bigint AS bytes,coalesce(max(d.seq),0) AS boundary,min(d.expires_at) AS earliest FROM post_relay_bodies b JOIN post_relay_dead_letters d USING(receipt) WHERE b.profile_id=$1 AND b.link_id=$2 AND b.generation=$3 AND d.expires_at>$4 AND ($5::text IS NULL OR b.receipt=$5)").bind(r.profile_id.as_ref()).bind(&link.id).bind(link.generation).bind(now).bind(&r.receipt).fetch_one(&mut **c).await.convert_error()?;
        Ok(OperatorPreview {
            count: row.get("n"),
            bytes: row.get("bytes"),
            boundary: row.get("boundary"),
            earliest_expiry: row.get("earliest"),
        })
    }
    pub(super) async fn preview(
        &self,
        c: &mut PostgresConnection,
        r: &OperatorRequest,
    ) -> error_stack::Result<OperatorPreview, KernelError> {
        Self::validate_operation(r)?;
        let g = self.gate(c, &r.profile_id).await?;
        let link = Self::operation_link(&g, r.action)?;
        let now = self.now(c).await?;
        self.selection(c, r, link, now).await
    }
    pub(super) async fn begin(
        &self,
        c: &mut PostgresConnection,
        r: &OperatorRequest,
    ) -> error_stack::Result<OperatorPreview, KernelError> {
        Self::validate_operation(r)?;
        let g = self.gate(c, &r.profile_id).await?;
        let link = Self::operation_link(&g, r.action)?;
        let (q, p) = self.quota(c, &g).await?;
        let now = self.now(c).await?;
        let action = match r.action {
            OperatorAction::Redeliver => "redeliver",
            OperatorAction::Discard => "discard",
        };
        if let Some(old) = sqlx::query("SELECT * FROM post_relay_operations WHERE id=$1")
            .bind(&r.id)
            .fetch_optional(&mut **c)
            .await
            .convert_error()?
        {
            if old.get::<i64, _>("profile_id") != g.profile
                || old.get::<String, _>("link_id") != link.id
                || old.get::<i64, _>("generation") != link.generation
                || old.get::<String, _>("action") != action
                || old.get::<String, _>("operator") != r.operator
                || old.get::<String, _>("reason") != r.reason
                || old.get::<Option<String>, _>("receipt") != r.receipt
                || old.get::<OffsetDateTime, _>("expires_at") <= now
            {
                return Err(Report::new(KernelError::Concurrency));
            }
            return Ok(OperatorPreview {
                count: old.get("selected_count"),
                bytes: old.get("selected_bytes"),
                boundary: old.get("boundary"),
                earliest_expiry: None,
            });
        }
        // This protected partition is not spent by ordinary admission; full body capacity cannot
        // prevent an explicit recovery/discard from obtaining its aggregate audit row.
        if q.operator_rows >= 100000 || p.operator_rows >= 1000 {
            return Err(Report::new(KernelError::Rejected)
                .attach_printable("post relay operation audit capacity"));
        }
        let selected = self.selection(c, r, link, now).await?;
        sqlx::query("INSERT INTO post_relay_operations(id,profile_id,link_id,generation,action,operator,reason,receipt,boundary,selected_count,selected_bytes,created_at,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)").bind(&r.id).bind(g.profile).bind(&link.id).bind(link.generation).bind(action).bind(&r.operator).bind(&r.reason).bind(&r.receipt).bind(selected.boundary).bind(selected.count).bind(selected.bytes).bind(now).bind(now+QUARANTINE_TTL).execute(&mut **c).await.convert_error()?;
        sqlx::query("UPDATE post_relay_quota SET operator_rows=operator_rows+1 WHERE singleton")
            .execute(&mut **c)
            .await
            .convert_error()?;
        sqlx::query(
            "UPDATE post_relay_profile_quota SET operator_rows=operator_rows+1 WHERE profile_id=$1",
        )
        .bind(g.profile)
        .execute(&mut **c)
        .await
        .convert_error()?;
        Ok(selected)
    }
    pub(super) async fn operate(
        &self,
        c: &mut PostgresConnection,
        id: &str,
        discard: bool,
    ) -> error_stack::Result<OperatorBatch, KernelError> {
        tx(c)?;
        let profile: i64 =
            sqlx::query_scalar("SELECT profile_id FROM post_relay_operations WHERE id=$1")
                .bind(id)
                .fetch_optional(&mut **c)
                .await
                .convert_error()?
                .ok_or_else(|| Report::new(KernelError::NotFound))?;
        let g = self.gate(c, &ProfileId::new(profile)).await?;
        let link = Self::operation_link(
            &g,
            if discard {
                OperatorAction::Discard
            } else {
                OperatorAction::Redeliver
            },
        )?;
        self.quota(c, &g).await?;
        let now = self.now(c).await?;
        // Global quota serializes selection/cursor updates. Audit write occurs after body/job work.
        let op = sqlx::query("SELECT * FROM post_relay_operations WHERE id=$1")
            .bind(id)
            .fetch_one(&mut **c)
            .await
            .convert_error()?;
        if op.get::<String, _>("link_id") != link.id
            || op.get::<i64, _>("generation") != link.generation
            || op.get::<OffsetDateTime, _>("expires_at") <= now
            || op.get::<String, _>("action") != if discard { "discard" } else { "redeliver" }
        {
            return Err(Report::new(KernelError::PermissionDenied));
        }
        let boundary: i64 = op.get("boundary");
        let cursor: i64 = op.get("cursor");
        let rows=sqlx::query("SELECT b.receipt,d.seq,b.logical_bytes FROM post_relay_bodies b JOIN post_relay_dead_letters d USING(receipt) WHERE b.profile_id=$1 AND b.link_id=$2 AND b.generation=$3 AND d.expires_at>$4 AND d.seq>$5 AND d.seq<=$6 AND ($7::text IS NULL OR b.receipt=$7) ORDER BY d.seq FOR UPDATE OF b LIMIT 128").bind(g.profile).bind(&link.id).bind(link.generation).bind(now).bind(cursor).bind(boundary).bind(op.get::<Option<String>,_>("receipt")).fetch_all(&mut **c).await.convert_error()?;
        let mut changed = 0u16;
        for row in &rows {
            let receipt: String = row.get("receipt");
            if discard {
                // A prior authorization is allowed to finish, but its slot remains until its deadline.
                if let Some(job) = self.job(c, &receipt).await? {
                    if let Some(op) = job.operation_id {
                        sqlx::query(
                            "UPDATE post_relay_operations SET cancelled=cancelled+1 WHERE id=$1",
                        )
                        .bind(op)
                        .execute(&mut **c)
                        .await
                        .convert_error()?;
                    }
                }
                self.delta(c, g.profile, -1, -row.get::<i64, _>("logical_bytes"), 0, 0)
                    .await?;
                self.usage(c, &link.id, -1, -row.get::<i64, _>("logical_bytes"), 0)
                    .await?;
                sqlx::query("DELETE FROM post_relay_bodies WHERE receipt=$1")
                    .bind(&receipt)
                    .execute(&mut **c)
                    .await
                    .convert_error()?;
                changed += 1;
            } else {
                changed+=sqlx::query("INSERT INTO post_relay_jobs(receipt,due_at,operation_id) VALUES($1,$2,$3) ON CONFLICT(receipt) DO NOTHING").bind(&receipt).bind(now).bind(id).execute(&mut **c).await.convert_error()?.rows_affected() as u16;
            }
        }
        let complete = rows.len() < usize::from(MAX_BATCH);
        let next = if complete {
            boundary
        } else {
            rows.last().unwrap().get("seq")
        };
        let sql = if discard {
            "UPDATE post_relay_operations SET cursor=$2,discarded=discarded+$3,skipped=skipped+$4 WHERE id=$1"
        } else {
            "UPDATE post_relay_operations SET cursor=$2,staged=staged+$3,skipped=skipped+$4 WHERE id=$1"
        };
        sqlx::query(sql)
            .bind(id)
            .bind(next)
            .bind(i64::from(changed))
            .bind(rows.len() as i64 - i64::from(changed))
            .execute(&mut **c)
            .await
            .convert_error()?;
        sqlx::query("DELETE FROM post_relay_link_usage WHERE link_id=$1 AND body_count=0")
            .bind(&link.id)
            .execute(&mut **c)
            .await
            .convert_error()?;
        Ok(OperatorBatch {
            selected: rows.len() as u16,
            changed,
            complete,
        })
    }
}
