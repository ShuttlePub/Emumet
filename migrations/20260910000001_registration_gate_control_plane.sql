-- 登録ゲート control plane (D15 / E15a)。
-- 状態は singleton 行 (id = 1)。revision は楽観ロック用で、制御状態 (admin/override/latch/episode)
-- が変わる遷移でのみ +1 する。監査は遷移ごとに 1 件 (revision_after が一意)、
-- 通知イベントは (revision, kind) が一意で、状態更新と同一 transaction で保存する。
CREATE TABLE "registration_gate_state" (
  "id" SMALLINT PRIMARY KEY CHECK ("id" = 1),
  "admin_state" TEXT NOT NULL DEFAULT 'closed',
  "override_active" BOOLEAN NOT NULL DEFAULT FALSE,
  "override_activated_by" BIGINT,
  "override_activated_at" TIMESTAMPTZ,
  "config_lost_latch" BOOLEAN NOT NULL DEFAULT FALSE,
  "health_episode" TEXT NOT NULL DEFAULT 'none',
  "email_configured" BOOLEAN NOT NULL DEFAULT FALSE,
  "email_probe" TEXT NOT NULL DEFAULT 'unknown',
  "bot_configured" BOOLEAN NOT NULL DEFAULT FALSE,
  "bot_probe" TEXT NOT NULL DEFAULT 'unknown',
  "revision" BIGINT NOT NULL DEFAULT 0,
  "updated_at" TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT chk_registration_gate_admin_state CHECK (admin_state IN ('closed', 'open')),
  CONSTRAINT chk_registration_gate_health_episode
    CHECK (health_episode IN ('none', 'outage', 'degraded')),
  CONSTRAINT chk_registration_gate_email_probe
    CHECK (email_probe IN ('healthy', 'unhealthy', 'unknown')),
  CONSTRAINT chk_registration_gate_bot_probe
    CHECK (bot_probe IN ('healthy', 'unhealthy', 'unknown')),
  CONSTRAINT chk_registration_gate_override_columns CHECK (
    (override_active AND override_activated_by IS NOT NULL AND override_activated_at IS NOT NULL)
    OR (NOT override_active AND override_activated_by IS NULL AND override_activated_at IS NULL)
  ),
  CONSTRAINT chk_registration_gate_override_excludes_latch_and_episode
    CHECK (NOT override_active OR (NOT config_lost_latch AND health_episode = 'none'))
);
INSERT INTO "registration_gate_state" ("id") VALUES (1);

CREATE TABLE "registration_gate_audit" (
  "id" BIGINT PRIMARY KEY,
  "revision_before" BIGINT NOT NULL,
  "revision_after" BIGINT NOT NULL UNIQUE,
  "action" TEXT NOT NULL,
  "actor_auth_account_id" BIGINT,
  "warning_acknowledged" BOOLEAN NOT NULL DEFAULT FALSE,
  "before_state" JSONB NOT NULL,
  "after_state" JSONB NOT NULL,
  "observation" JSONB NOT NULL,
  "created_at" TIMESTAMPTZ NOT NULL,
  CONSTRAINT chk_registration_gate_audit_action CHECK (
    action IN ('open', 'close', 'enable_override', 'revoke_override', 'observation')
  ),
  CONSTRAINT chk_registration_gate_audit_actor CHECK (
    (action = 'observation' AND actor_auth_account_id IS NULL)
    OR (action <> 'observation' AND actor_auth_account_id IS NOT NULL)
  )
);

CREATE TABLE "registration_gate_events" (
  "id" BIGINT PRIMARY KEY,
  "kind" TEXT NOT NULL,
  "revision" BIGINT NOT NULL,
  "payload" JSONB NOT NULL,
  "created_at" TIMESTAMPTZ NOT NULL,
  CONSTRAINT chk_registration_gate_event_kind CHECK (
    kind IN (
      'outage_started', 'outage_recovered', 'protection_degraded',
      'protection_lost_stopped', 'override_started', 'override_revoked'
    )
  ),
  CONSTRAINT uq_registration_gate_event_revision_kind UNIQUE ("revision", "kind")
);

CREATE TABLE "registration_gate_event_reads" (
  "admin_auth_account_id" BIGINT NOT NULL REFERENCES "auth_accounts" ("id") ON DELETE CASCADE,
  "event_id" BIGINT NOT NULL REFERENCES "registration_gate_events" ("id") ON DELETE CASCADE,
  "read_at" TIMESTAMPTZ NOT NULL,
  PRIMARY KEY ("admin_auth_account_id", "event_id")
);
