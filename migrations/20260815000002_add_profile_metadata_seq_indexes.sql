-- ADR 0006 Stage 7: tailing reads for profile and metadata projections.
CREATE INDEX IF NOT EXISTS idx_profile_events_seq ON profile_events (seq);
CREATE INDEX IF NOT EXISTS idx_metadata_events_seq ON metadata_events (seq);
