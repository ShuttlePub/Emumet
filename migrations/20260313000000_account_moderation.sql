ALTER TABLE accounts ADD COLUMN suspended_at TIMESTAMPTZ;
ALTER TABLE accounts ADD COLUMN suspend_expires_at TIMESTAMPTZ;
ALTER TABLE accounts ADD COLUMN suspend_reason TEXT;
ALTER TABLE accounts ADD COLUMN banned_at TIMESTAMPTZ;
ALTER TABLE accounts ADD COLUMN ban_reason TEXT;

ALTER TABLE accounts ADD CONSTRAINT chk_suspend_fields
    CHECK ((suspended_at IS NULL AND suspend_reason IS NULL AND suspend_expires_at IS NULL)
        OR (suspended_at IS NOT NULL AND suspend_reason IS NOT NULL));

ALTER TABLE accounts ADD CONSTRAINT chk_ban_fields
    CHECK ((banned_at IS NULL AND ban_reason IS NULL)
        OR (banned_at IS NOT NULL AND ban_reason IS NOT NULL));

ALTER TABLE accounts ADD CONSTRAINT chk_not_both_suspended_and_banned
    CHECK (NOT (suspended_at IS NOT NULL AND banned_at IS NOT NULL));
