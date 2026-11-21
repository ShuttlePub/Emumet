-- OFFLINE PROBE ONLY: psql "$DISPOSABLE_DATABASE_URL" -X -f this-file.sql
-- ~4GiB uncompressed TOAST + attempts + WAL. Never use a production database.
\set ON_ERROR_STOP on
\timing on
CREATE SCHEMA q1_cancel_probe;
SET search_path=q1_cancel_probe;
CREATE TABLE links(id bigint PRIMARY KEY,state text NOT NULL);
CREATE TABLE bodies(id bigint PRIMARY KEY,link bigint REFERENCES links(id),envelope bytea NOT NULL);
ALTER TABLE bodies ALTER COLUMN envelope SET STORAGE EXTERNAL;
CREATE INDEX ON bodies(link);
CREATE TABLE attempts(body bigint REFERENCES bodies(id) ON DELETE CASCADE,n int,PRIMARY KEY(body,n));
INSERT INTO links SELECT n,'active' FROM generate_series(1,8) n;
INSERT INTO bodies SELECT n,1+(n-1)/2000,convert_to(repeat('x',264339+CASE WHEN n%2000=0 THEN 912 ELSE 0 END),'UTF8') FROM generate_series(1,16000) n;
INSERT INTO attempts SELECT id,n FROM bodies CROSS JOIN generate_series(1,16) n;
SELECT link,count(*),sum(octet_length(envelope))+count(*)*4096 AS logical_bytes FROM bodies GROUP BY link ORDER BY link;
BEGIN;
SET LOCAL lock_timeout='2s'; SET LOCAL statement_timeout='3s'; SET LOCAL idle_in_transaction_session_timeout='5s';
UPDATE links SET state='unlinked' WHERE id=1;
DELETE FROM bodies WHERE link=1;
COMMIT;
BEGIN;
SET LOCAL lock_timeout='2s'; SET LOCAL statement_timeout='3s'; SET LOCAL idle_in_transaction_session_timeout='5s';
UPDATE links SET state='unlinked';
-- Expected to exceed the bound on the measured host. Preserve the error/elapsed output.
\set ON_ERROR_STOP off
DELETE FROM bodies WHERE link>1;
\set ON_ERROR_STOP on
COMMIT;
DROP SCHEMA q1_cancel_probe CASCADE;
