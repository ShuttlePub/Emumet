ALTER TABLE outbox_activities
ADD COLUMN IF NOT EXISTS delivered_at TIMESTAMPTZ,
ADD COLUMN IF NOT EXISTS attempted_at TIMESTAMPTZ,
ADD COLUMN IF NOT EXISTS error TEXT;

CREATE INDEX IF NOT EXISTS idx_outbox_activities_pending ON outbox_activities(account_id) WHERE delivered_at IS NULL;
