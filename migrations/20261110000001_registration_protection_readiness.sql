-- 登録保護の readiness 記録・登録前 email challenge/proof・送信 rate limit (D15 / E15c)。
-- 平文の code / proof / email / secret は保存しない (hash と fingerprint のみ)。

CREATE TABLE registration_protection_readiness (
    protection TEXT PRIMARY KEY CHECK (protection IN ('email', 'bot')),
    provider TEXT NOT NULL CHECK (provider IN ('smtp', 'turnstile')),
    enabled BOOLEAN NOT NULL,
    generation TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    checked_at TIMESTAMPTZ NOT NULL,
    check_outcome TEXT NOT NULL CHECK (check_outcome IN (
        'business_success', 'accepted', 'configuration_invalid', 'attempt_rejected',
        'temporarily_unavailable'
    )),
    check_reason TEXT NOT NULL,
    check_transport TEXT NOT NULL,
    last_success_at TIMESTAMPTZ,
    success_valid_until TIMESTAMPTZ,
    status TEXT NOT NULL CHECK (status IN (
        'configuration_invalid', 'operational', 'temporarily_unavailable', 'unverified'
    )),
    reason TEXT NOT NULL,
    valid_until TIMESTAMPTZ,
    -- 配送範囲 (canary) の未復旧の障害。同一 generation の間、配送範囲の成功まで接続 probe では解除されない。
    delivery_block_outcome TEXT CHECK (delivery_block_outcome IN ('configuration_invalid', 'temporarily_unavailable')),
    delivery_block_reason TEXT,
    delivery_block_transport TEXT,
    delivery_block_at TIMESTAMPTZ,
    CHECK ((last_success_at IS NULL) = (success_valid_until IS NULL)),
    CHECK (
        (delivery_block_outcome IS NULL) = (delivery_block_reason IS NULL)
        AND (delivery_block_outcome IS NULL) = (delivery_block_transport IS NULL)
        AND (delivery_block_outcome IS NULL) = (delivery_block_at IS NULL)
    )
);

CREATE TABLE registration_email_challenges (
    id TEXT PRIMARY KEY,
    purpose TEXT NOT NULL CHECK (purpose IN ('registration', 'canary')),
    flow_id TEXT NOT NULL,
    email_hash TEXT NOT NULL,
    generation TEXT NOT NULL,
    code_hash TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    max_attempts INTEGER NOT NULL CHECK (max_attempts > 0),
    status TEXT NOT NULL CHECK (status IN ('pending', 'confirmed', 'invalidated')),
    created_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    flow_expires_at TIMESTAMPTZ NOT NULL,
    confirmed_at TIMESTAMPTZ,
    proof_hash TEXT,
    proof_expires_at TIMESTAMPTZ,
    consumed_at TIMESTAMPTZ,
    CHECK (status <> 'confirmed' OR confirmed_at IS NOT NULL),
    CHECK (proof_hash IS NULL OR (status = 'confirmed' AND purpose = 'registration')),
    CHECK (consumed_at IS NULL OR proof_hash IS NOT NULL)
);

-- proof は 1 つの challenge にだけ属する (hash の一意性で消費先を一意に引く)。
CREATE UNIQUE INDEX registration_email_challenges_proof_hash_key
    ON registration_email_challenges (proof_hash) WHERE proof_hash IS NOT NULL;
CREATE INDEX registration_email_challenges_expires_at_idx
    ON registration_email_challenges (expires_at);

CREATE TABLE registration_email_send_log (
    id BIGSERIAL PRIMARY KEY,
    email_hash TEXT NOT NULL,
    flow_id TEXT NOT NULL,
    source_hash TEXT NOT NULL,
    sent_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX registration_email_send_log_email_idx ON registration_email_send_log (email_hash, sent_at);
CREATE INDEX registration_email_send_log_flow_idx ON registration_email_send_log (flow_id, sent_at);
CREATE INDEX registration_email_send_log_source_idx ON registration_email_send_log (source_hash, sent_at);
CREATE INDEX registration_email_send_log_sent_at_idx ON registration_email_send_log (sent_at);
