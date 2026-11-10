-- Singleton schedule plus a transaction-scoped advisory lock prevents concurrent
-- and sequential replicas duplicating the same periodic provider exchange.
CREATE TABLE registration_protection_schedule (
    id smallint PRIMARY KEY CHECK (id = 1),
    next_run_at timestamptz NOT NULL
);
