CREATE TABLE outbox_activities (
    id BIGSERIAL PRIMARY KEY,
    account_id BIGINT NOT NULL,
    activity_id TEXT UNIQUE NOT NULL,
    activity_type TEXT NOT NULL,
    object_json TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_outbox_activities_account_id ON outbox_activities(account_id);
CREATE INDEX idx_outbox_activities_created_at ON outbox_activities(created_at DESC);
