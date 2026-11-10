-- Evidence is produced by real public Kratos registration, not a health/config response.
CREATE TABLE registration_enforcement_probe_state (
    id smallint PRIMARY KEY CHECK (id = 1),
    generation text NOT NULL,
    verified boolean NOT NULL,
    checked_at timestamptz NOT NULL,
    valid_until timestamptz NOT NULL
);

CREATE TABLE registration_enforcement_probe_schedule (
    id smallint PRIMARY KEY CHECK (id = 1),
    next_probe_at timestamptz NOT NULL
);

CREATE TABLE registration_enforcement_canaries (
    probe_id text PRIMARY KEY CHECK (probe_id ~ '^[0-9a-f]{32}$'),
    target_key text NOT NULL,
    flow_id text NOT NULL,
    created_at timestamptz NOT NULL,
    received_at timestamptz,
    settled boolean NOT NULL DEFAULT false
);

CREATE INDEX registration_enforcement_canaries_pending
    ON registration_enforcement_canaries (created_at) WHERE NOT settled;
