WITH live AS (
 SELECT b.receipt,b.profile_id,b.link_id,b.logical_bytes,j.operation_id,j.receipt AS job_receipt,d.receipt AS dead_receipt FROM post_relay_bodies b
 JOIN profile_links l ON l.id=b.link_id LEFT JOIN post_relay_jobs j USING(receipt)
 LEFT JOIN post_relay_dead_letters d USING(receipt)
 WHERE l.state='active' AND l.generation=b.generation
), b AS (
 SELECT profile_id,count(*) AS n,sum(logical_bytes) AS bytes,
 count(*) FILTER(WHERE job_receipt IS NOT NULL AND operation_id IS NULL) AS jobs FROM live GROUP BY profile_id
), a AS (SELECT profile_id,count(*) AS n FROM post_relay_audits GROUP BY profile_id),
u AS (SELECT profile_id,count(*) AS n FROM post_relay_unlinked GROUP BY profile_id),
o AS (SELECT profile_id,count(*) AS n FROM post_relay_operations GROUP BY profile_id),
ids AS (SELECT profile_id FROM b UNION SELECT profile_id FROM a UNION SELECT profile_id FROM u UNION SELECT profile_id FROM o UNION SELECT profile_id FROM post_relay_profile_quota),
lu AS (SELECT link_id,count(*) AS n,sum(logical_bytes) AS bytes,count(*) FILTER(WHERE job_receipt IS NOT NULL AND operation_id IS NULL) AS jobs FROM live GROUP BY link_id)
SELECT
 NOT EXISTS(SELECT 1 FROM ids LEFT JOIN post_relay_profile_quota q USING(profile_id) LEFT JOIN b USING(profile_id) LEFT JOIN a USING(profile_id) LEFT JOIN u USING(profile_id) LEFT JOIN o USING(profile_id)
 WHERE q.profile_id IS NULL OR q.body_count<>coalesce(b.n,0) OR q.body_bytes<>coalesce(b.bytes,0)
 OR q.delivery_rows<>(q.delivery_state::int)+coalesce(b.jobs,0)*17+coalesce(a.n,0)
 OR q.delivery_bytes<>(q.delivery_state::int)*512+coalesce(b.jobs,0)*9216+coalesce(a.n,0)*1024
 OR q.unlinked_rows<>coalesce(u.n,0)+(NOT q.delivery_state)::int OR q.operator_rows<>coalesce(o.n,0))
 AND NOT EXISTS(SELECT 1 FROM lu FULL JOIN (SELECT * FROM post_relay_link_usage WHERE NOT released) l USING(link_id)
 WHERE coalesce(l.body_count,-1)<>coalesce(lu.n,0) OR coalesce(l.body_bytes,-1)<>coalesce(lu.bytes,0) OR coalesce(l.auto_jobs,-1)<>coalesce(lu.jobs,0))
 AND NOT EXISTS(SELECT 1 FROM live WHERE (job_receipt IS NULL AND dead_receipt IS NULL) OR (operation_id IS NOT NULL AND dead_receipt IS NULL) OR (operation_id IS NULL AND job_receipt IS NOT NULL AND dead_receipt IS NOT NULL))
 AND q.body_count=(SELECT count(*) FROM live) AND q.body_bytes=coalesce((SELECT sum(logical_bytes) FROM live),0)
 AND q.delivery_rows=64+(SELECT count(*) FROM post_relay_profile_quota WHERE delivery_state)+(SELECT count(*) FROM post_relay_audits)+17*(SELECT count(*) FROM live WHERE job_receipt IS NOT NULL AND operation_id IS NULL)
 AND q.delivery_bytes=65536+512*(SELECT count(*) FROM post_relay_profile_quota WHERE delivery_state)+1024*(SELECT count(*) FROM post_relay_audits)+9216*(SELECT count(*) FROM live WHERE job_receipt IS NOT NULL AND operation_id IS NULL)
 AND q.unlinked_rows=(SELECT count(*) FROM post_relay_unlinked)+(SELECT count(*) FROM post_relay_profile_quota WHERE NOT delivery_state) AND q.operator_rows=(SELECT count(*) FROM post_relay_operations)
FROM post_relay_quota q WHERE singleton
