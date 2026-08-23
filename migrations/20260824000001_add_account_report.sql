CREATE TABLE "account_report_events" (
  "id" BIGINT NOT NULL,
  "version" BIGINT NOT NULL,
  "event_name" TEXT NOT NULL,
  "data" JSONB NOT NULL,
  "occurred_at" TIMESTAMPTZ NOT NULL DEFAULT now(),
  "seq" BIGSERIAL,
  PRIMARY KEY ("id", "version")
);

CREATE TABLE "account_reports" (
  "id" BIGINT PRIMARY KEY,
  "target_account_id" BIGINT NOT NULL,
  "reported_by_account_id" BIGINT NOT NULL,
  "category" TEXT NOT NULL,
  "comment" TEXT NULL,
  "status" TEXT NOT NULL DEFAULT 'open',
  "resolution" TEXT NULL,
  "close_reason" TEXT NULL,
  "version" BIGINT NOT NULL,
  "nanoid" TEXT NOT NULL UNIQUE
);

CREATE INDEX IF NOT EXISTS idx_account_report_events_seq ON account_report_events (seq);
CREATE INDEX IF NOT EXISTS idx_account_reports_status ON account_reports (status);
CREATE INDEX IF NOT EXISTS idx_account_reports_target_account_id ON account_reports (target_account_id);
