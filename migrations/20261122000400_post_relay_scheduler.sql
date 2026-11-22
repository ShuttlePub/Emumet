-- S1 only: bounded active-Profile scheduling state, never a delivery terminal history.
CREATE TABLE post_relay_schedule (
    profile_id bigint PRIMARY KEY,
    last_claimed_at timestamptz NOT NULL
);
CREATE INDEX post_relay_schedule_age ON post_relay_schedule(last_claimed_at);

-- Only the bounded preparation window, not all pending bodies, participates in occupancy.
CREATE INDEX post_relay_preparing_lease ON post_relay_jobs(lease_until) WHERE authorized_at IS NULL AND lease_until IS NOT NULL;
