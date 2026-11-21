-- P1 only, additive/forward-only. No changes to existing Profile delivery.
CREATE TABLE post_relay_quota (
 singleton boolean PRIMARY KEY DEFAULT true CHECK (singleton),
 body_count bigint NOT NULL DEFAULT 0 CHECK(body_count BETWEEN 0 AND 100000),
 body_bytes bigint NOT NULL DEFAULT 0 CHECK(body_bytes BETWEEN 0 AND 21474836480),
 delivery_rows bigint NOT NULL DEFAULT 0 CHECK(delivery_rows BETWEEN 0 AND 1800000),
 delivery_bytes bigint NOT NULL DEFAULT 0 CHECK(delivery_bytes BETWEEN 0 AND 1887436800),
 unlinked_rows bigint NOT NULL DEFAULT 0 CHECK(unlinked_rows BETWEEN 0 AND 100000),
 operator_rows bigint NOT NULL DEFAULT 0 CHECK(operator_rows BETWEEN 0 AND 100000),
 tokens double precision NOT NULL DEFAULT 1000, token_at timestamptz NOT NULL DEFAULT clock_timestamp(),
 unlinked_tokens double precision NOT NULL DEFAULT 1000, unlinked_token_at timestamptz NOT NULL DEFAULT clock_timestamp(),
 consistent boolean NOT NULL DEFAULT true
);
INSERT INTO post_relay_quota(singleton,delivery_rows,delivery_bytes) VALUES(true,64,65536);
-- Independent partitions. Delivery reservations + buckets (512 bytes) + operations (1024)
-- total <=2M rows and (1800+49+98) MiB <2GiB.
CREATE TABLE post_relay_profile_quota (
 profile_id bigint PRIMARY KEY, owner_account_id bigint NOT NULL,
 delivery_state boolean NOT NULL DEFAULT false,
 body_count bigint NOT NULL DEFAULT 0 CHECK(body_count BETWEEN 0 AND 2000),
 body_bytes bigint NOT NULL DEFAULT 0 CHECK(body_bytes BETWEEN 0 AND 536870912),
 delivery_rows bigint NOT NULL DEFAULT 0 CHECK(delivery_rows BETWEEN 0 AND 48000),
 delivery_bytes bigint NOT NULL DEFAULT 0 CHECK(delivery_bytes BETWEEN 0 AND 62914560),
 unlinked_rows bigint NOT NULL DEFAULT 0 CHECK(unlinked_rows BETWEEN 0 AND 1000),
 operator_rows bigint NOT NULL DEFAULT 0 CHECK(operator_rows BETWEEN 0 AND 1000),
 tokens double precision NOT NULL DEFAULT 200, token_at timestamptz NOT NULL DEFAULT clock_timestamp(),
 touched_at timestamptz NOT NULL DEFAULT clock_timestamp()
);
CREATE INDEX post_relay_profile_quota_idle ON post_relay_profile_quota(touched_at) WHERE body_count=0;
CREATE TABLE post_relay_capacity (
 singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton),
 free_bytes bigint CHECK(free_bytes>=0), total_bytes bigint CHECK(total_bytes>0),
 sampled_at timestamptz, accepting boolean NOT NULL DEFAULT false,
 CHECK(free_bytes<=total_bytes)
);
INSERT INTO post_relay_capacity(singleton) VALUES(true);
-- Fixed permit slots survive cancellation until the original send deadline. Removing a job
-- must not free a slot still used by an already-authorized HTTP operation.
CREATE TABLE post_relay_slots (
 id smallint PRIMARY KEY CHECK(id BETWEEN 1 AND 16), origin varchar(2048),
 token bigint, until_at timestamptz
);
INSERT INTO post_relay_slots(id) SELECT generate_series(1,16);
CREATE SEQUENCE post_relay_fence AS bigint NO CYCLE;
CREATE TABLE post_relay_link_usage (
 link_id text PRIMARY KEY REFERENCES profile_links(id), profile_id bigint NOT NULL,
 body_count bigint NOT NULL DEFAULT 0 CHECK(body_count>=0),
 body_bytes bigint NOT NULL DEFAULT 0 CHECK(body_bytes>=0),
 auto_jobs bigint NOT NULL DEFAULT 0 CHECK(auto_jobs>=0),
 released boolean NOT NULL DEFAULT false
);
CREATE TABLE post_relay_bodies (
 receipt varchar(128) PRIMARY KEY CHECK(octet_length(receipt) BETWEEN 1 AND 128),
 seq bigint GENERATED ALWAYS AS IDENTITY UNIQUE,
 profile_id bigint NOT NULL, link_id text NOT NULL REFERENCES profile_links(id),
 generation bigint NOT NULL CHECK(generation>0), origin varchar(2048) NOT NULL,
 received_at timestamptz NOT NULL, automatic_expires_at timestamptz NOT NULL,
 envelope bytea NOT NULL CHECK(octet_length(envelope) BETWEEN 1 AND 2097152),
 logical_bytes bigint NOT NULL CHECK(logical_bytes=octet_length(envelope)+4096),
 CHECK(automatic_expires_at=received_at+interval '72 hours')
);
CREATE INDEX post_relay_bodies_link ON post_relay_bodies(link_id,seq);
CREATE INDEX post_relay_bodies_profile ON post_relay_bodies(profile_id,seq);
CREATE TABLE post_relay_operations (
 id varchar(64) PRIMARY KEY, profile_id bigint NOT NULL, link_id text NOT NULL REFERENCES profile_links(id),
 generation bigint NOT NULL, action text NOT NULL CHECK(action IN ('redeliver','discard')),
 operator varchar(128) NOT NULL CHECK(octet_length(operator) BETWEEN 1 AND 128),
 reason varchar(128) NOT NULL CHECK(octet_length(reason) BETWEEN 1 AND 128),
 receipt varchar(128), boundary bigint NOT NULL, cursor bigint NOT NULL DEFAULT 0,
 selected_count bigint NOT NULL, selected_bytes bigint NOT NULL,
 staged bigint NOT NULL DEFAULT 0, succeeded bigint NOT NULL DEFAULT 0,
 failed bigint NOT NULL DEFAULT 0, cancelled bigint NOT NULL DEFAULT 0, discarded bigint NOT NULL DEFAULT 0, skipped bigint NOT NULL DEFAULT 0,
 created_at timestamptz NOT NULL, expires_at timestamptz NOT NULL
);
CREATE INDEX post_relay_operations_expiry ON post_relay_operations(expires_at,profile_id);
CREATE TABLE post_relay_jobs (
 receipt varchar(128) PRIMARY KEY REFERENCES post_relay_bodies(receipt) ON DELETE CASCADE,
 due_at timestamptz NOT NULL, token bigint NOT NULL DEFAULT 0,
 lease_until timestamptz, attempt_id bigint, authorized_at timestamptz, send_deadline timestamptz,
 operation_id varchar(64) REFERENCES post_relay_operations(id),
 total_attempts bigint NOT NULL DEFAULT 0, first_attempt_at timestamptz, last_attempt_at timestamptz,
 last_status smallint CHECK(last_status BETWEEN 100 AND 599),
 CHECK((authorized_at IS NULL AND send_deadline IS NULL AND attempt_id IS NULL)
    OR (authorized_at IS NOT NULL AND send_deadline>authorized_at AND attempt_id IS NOT NULL))
);
CREATE INDEX post_relay_jobs_due ON post_relay_jobs(due_at,receipt);
CREATE INDEX post_relay_jobs_lease ON post_relay_jobs(lease_until);
CREATE TABLE post_relay_attempts (
 receipt varchar(128) REFERENCES post_relay_jobs(receipt) ON DELETE CASCADE,
 number bigint NOT NULL, at timestamptz NOT NULL, status smallint CHECK(status BETWEEN 100 AND 599),
 reason text NOT NULL CHECK(reason IN ('retry','permanent','success','expired')),
 PRIMARY KEY(receipt,number)
);
CREATE TABLE post_relay_dead_letters (
 seq bigint GENERATED ALWAYS AS IDENTITY UNIQUE,
 receipt varchar(128) PRIMARY KEY REFERENCES post_relay_bodies(receipt) ON DELETE CASCADE,
 terminal_at timestamptz NOT NULL, expires_at timestamptz NOT NULL,
 reason text NOT NULL CHECK(reason IN ('permanent','expired')),
 CHECK(expires_at=terminal_at+interval '720 hours')
);
CREATE INDEX post_relay_dead_letters_expiry ON post_relay_dead_letters(expires_at,receipt);
CREATE TABLE post_relay_audits (
 receipt varchar(128) PRIMARY KEY, profile_id bigint NOT NULL, link_id text NOT NULL,
 reason text NOT NULL CHECK(reason IN ('permanent','expired')),
 total_attempts bigint NOT NULL, first_attempt_at timestamptz, last_attempt_at timestamptz,
 last_status smallint CHECK(last_status BETWEEN 100 AND 599),
 terminal_at timestamptz NOT NULL, expires_at timestamptz NOT NULL,
 CHECK(expires_at=terminal_at+interval '720 hours')
);
CREATE INDEX post_relay_audits_expiry ON post_relay_audits(expires_at,profile_id);
CREATE TABLE post_relay_unlinked (
 profile_id bigint NOT NULL, reason text NOT NULL CHECK(reason='no_home'), bucket timestamptz NOT NULL,
 count bigint NOT NULL CHECK(count>0), first_at timestamptz NOT NULL, last_at timestamptz NOT NULL,
 expires_at timestamptz NOT NULL, PRIMARY KEY(profile_id,reason,bucket)
);
CREATE INDEX post_relay_unlinked_expiry ON post_relay_unlinked(expires_at,profile_id);
CREATE TABLE post_relay_counters (
 singleton boolean PRIMARY KEY DEFAULT true CHECK(singleton),
 succeeded bigint NOT NULL DEFAULT 0, cancelled bigint NOT NULL DEFAULT 0,
 permanent bigint NOT NULL DEFAULT 0, expired bigint NOT NULL DEFAULT 0,
 retries bigint NOT NULL DEFAULT 0, held bigint NOT NULL DEFAULT 0,
 purged bigint NOT NULL DEFAULT 0, last_purge_at timestamptz
);
INSERT INTO post_relay_counters(singleton) VALUES(true);
CREATE FUNCTION post_relay_immutable_body() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN RAISE EXCEPTION 'post relay bodies are immutable'; END $$;
CREATE TRIGGER post_relay_immutable_body BEFORE UPDATE ON post_relay_bodies
 FOR EACH ROW EXECUTE FUNCTION post_relay_immutable_body();
