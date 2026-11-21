-- S1 only: bounded active-Profile scheduling state, never a delivery terminal history.
CREATE TABLE post_relay_schedule (
    profile_id bigint PRIMARY KEY,
    last_claimed_at timestamptz NOT NULL
);
CREATE INDEX post_relay_schedule_age ON post_relay_schedule(last_claimed_at);
