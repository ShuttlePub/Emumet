-- Post-relay alert conditions (plan 6.6), evaluated by the deployment monitor every 30 s as a
-- read-only query (no locks beyond a snapshot). One row per FIRING alert:
--   alert    fixed name (never a receipt, actor, Activity ID, origin, body or credential)
--   severity 'warning' | 'critical'
--   value    the measured ratio / count / age in seconds
-- Alerts that need Emumet's own log lines (fixed messages, no payload) are listed in
-- docs/post-relay-operations.md ("T1 alarm runbook"). The reservation check is the read-only
-- statement driver/src/database/postgres/post_relay/consistency.sql (see alerts.sh).
WITH q AS (SELECT * FROM post_relay_quota WHERE singleton),
c AS (SELECT * FROM post_relay_capacity WHERE singleton),
pq AS (
  SELECT coalesce(max(body_count::float8 / 2000), 0) AS body_count,
         coalesce(max(body_bytes::float8 / 536870912), 0) AS body_bytes,
         coalesce(max(unlinked_rows::float8 / 1000), 0) AS audit_rows,
         coalesce(max(operator_rows::float8 / 1000), 0) AS operator_rows,
         coalesce(max(delivery_rows::float8 / 48000), 0) AS delivery_rows,
         coalesce(max(delivery_bytes::float8 / 62914560), 0) AS delivery_bytes
  FROM post_relay_profile_quota
),
ratios(name, ratio) AS (
  SELECT 'global_body_count', body_count::float8 / 100000 FROM q
  UNION ALL SELECT 'global_body_bytes', body_bytes::float8 / 21474836480 FROM q
  UNION ALL SELECT 'global_delivery_rows', delivery_rows::float8 / 1800000 FROM q
  UNION ALL SELECT 'global_delivery_bytes', delivery_bytes::float8 / 1887436800 FROM q
  UNION ALL SELECT 'global_audit_rows', unlinked_rows::float8 / 100000 FROM q
  UNION ALL SELECT 'global_operator_rows', operator_rows::float8 / 100000 FROM q
  UNION ALL SELECT 'profile_body_count', body_count FROM pq
  UNION ALL SELECT 'profile_body_bytes', body_bytes FROM pq
  UNION ALL SELECT 'profile_audit_rows', audit_rows FROM pq
  UNION ALL SELECT 'profile_operator_rows', operator_rows FROM pq
  UNION ALL SELECT 'profile_delivery_rows', delivery_rows FROM pq
  UNION ALL SELECT 'profile_delivery_bytes', delivery_bytes FROM pq
)
SELECT 'quota_' || name AS alert,
       CASE WHEN ratio >= 0.95 THEN 'critical' ELSE 'warning' END AS severity,
       ratio AS value
FROM ratios WHERE ratio >= 0.8
UNION ALL
SELECT 'physical_capacity_missing_or_stale', 'critical',
       coalesce(extract(epoch FROM clock_timestamp() - sampled_at), 'Infinity'::float8)
FROM c
WHERE sampled_at IS NULL OR sampled_at < clock_timestamp() - interval '90 seconds' OR sampled_at > clock_timestamp()
UNION ALL
SELECT 'physical_capacity_threshold', 'critical', free_bytes::float8 / total_bytes
FROM c
WHERE sampled_at >= clock_timestamp() - interval '90 seconds' AND sampled_at <= clock_timestamp() AND NOT accepting
UNION ALL
SELECT 'reservation_flag_inconsistent', 'critical', 1 FROM q WHERE NOT consistent
UNION ALL
SELECT 'purge_delay', 'warning', n::float8 FROM (
  SELECT (SELECT count(*) FROM post_relay_dead_letters WHERE expires_at < clock_timestamp() - interval '5 minutes')
       + (SELECT count(*) FROM post_relay_audits WHERE expires_at < clock_timestamp() - interval '5 minutes')
       + (SELECT count(*) FROM post_relay_unlinked WHERE expires_at < clock_timestamp() - interval '5 minutes')
       + (SELECT count(*) FROM post_relay_operations WHERE expires_at < clock_timestamp() - interval '5 minutes')
       + (SELECT count(*) FROM post_relay_jobs j JOIN post_relay_bodies b USING (receipt)
            WHERE j.operation_id IS NULL AND b.automatic_expires_at < clock_timestamp() - interval '5 minutes') AS n
) x WHERE n > 0
UNION ALL
SELECT 'cancelled_body_backlog', 'warning', count(*)::float8
FROM post_relay_bodies b JOIN profile_links l ON l.id = b.link_id
WHERE (l.state <> 'active' OR l.generation <> b.generation) AND l.unlinked_at < clock_timestamp() - interval '5 minutes'
HAVING count(*) > 0
UNION ALL
SELECT 'deadline_near_backlog', 'warning', count(*)::float8
FROM post_relay_jobs j JOIN post_relay_bodies b USING (receipt)
WHERE j.operation_id IS NULL AND b.automatic_expires_at < clock_timestamp() + interval '6 hours'
HAVING count(*) > 0
UNION ALL
SELECT 'repeated_401', 'critical', count(*)::float8
FROM post_relay_jobs WHERE last_status = 401 AND total_attempts >= 3
HAVING count(*) > 0
UNION ALL
SELECT 'db_lock_wait', 'warning', count(*)::float8
FROM pg_stat_activity
WHERE datname = current_database() AND wait_event_type = 'Lock' AND clock_timestamp() - state_change > interval '1 second'
HAVING count(*) > 0
UNION ALL
SELECT 'db_connection_pressure', 'warning',
       (SELECT count(*) FROM pg_stat_activity)::float8 / current_setting('max_connections')::float8
WHERE (SELECT count(*) FROM pg_stat_activity)::float8 / current_setting('max_connections')::float8 >= 0.8
UNION ALL
SELECT 'projector_stalled', 'critical', ((SELECT max(seq) FROM account_events) - k.last_seq)::float8
FROM projection_checkpoints k
WHERE k.projector_name = 'account_projector'
  AND k.updated_at < clock_timestamp() - interval '60 seconds'
  AND (SELECT max(seq) FROM account_events) > k.last_seq
ORDER BY 1;
