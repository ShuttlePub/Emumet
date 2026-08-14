-- Generic checkpoint table for transactional log tailing projectors
-- (ADR 0006 decision 4). last_seq is monotonic: it never regresses even when
-- a window re-read delivers an older max seq.
CREATE TABLE projection_checkpoints (
    projector_name TEXT PRIMARY KEY,
    last_seq BIGINT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

-- Tailing reads account_events by global seq order (ADR 0006 Stage 1 added the
-- column; the index makes the window read seq > checkpoint - W cheap).
CREATE INDEX idx_account_events_seq ON account_events (seq);
