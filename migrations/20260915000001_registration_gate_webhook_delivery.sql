-- 登録ゲート通知の外部 Webhook 配送 (D15 / E15w)。
-- 配送記録は通知イベント (registration_gate_events) を読み取って作り、イベント・状態・既読は変更しない。
-- 通知イベントは追記専用なので外部キーは張らない (E15a のテストが events を TRUNCATE できるようにする)。
-- 配送対象は通知イベントのみで、kind は通知イベント種別に限定する (配送の失敗から配送対象を作らない)。

-- Webhook が最初に有効と観測された時点の最新イベント ID。これ以前のイベントは配送しない。
CREATE TABLE "registration_gate_webhook_state" (
  "id" SMALLINT PRIMARY KEY CHECK ("id" = 1),
  "baseline_event_id" BIGINT NOT NULL,
  "enabled_since" TIMESTAMPTZ NOT NULL
);

CREATE TABLE "registration_gate_webhook_deliveries" (
  "event_id" BIGINT PRIMARY KEY,
  "kind" TEXT NOT NULL,
  "payload" TEXT NOT NULL,
  "status" TEXT NOT NULL,
  "attempt_count" INTEGER NOT NULL DEFAULT 0,
  "cycle_attempt_count" INTEGER NOT NULL DEFAULT 0,
  "cycle_started_at" TIMESTAMPTZ NOT NULL,
  "next_attempt_at" TIMESTAMPTZ NOT NULL,
  "last_attempt_at" TIMESTAMPTZ,
  "last_result" TEXT,
  "last_http_status" INTEGER,
  "failure_reason" TEXT,
  "lease_owner" TEXT,
  "lease_expires_at" TIMESTAMPTZ,
  "delivered_at" TIMESTAMPTZ,
  "created_at" TIMESTAMPTZ NOT NULL,
  "updated_at" TIMESTAMPTZ NOT NULL,
  CONSTRAINT chk_webhook_delivery_kind CHECK (
    kind IN (
      'outage_started', 'outage_recovered', 'protection_degraded',
      'protection_lost_stopped', 'override_started', 'override_revoked'
    )
  ),
  CONSTRAINT chk_webhook_delivery_status CHECK (
    status IN ('pending', 'in_flight', 'retrying', 'delivered', 'failed')
  ),
  CONSTRAINT chk_webhook_delivery_lease CHECK (
    (status = 'in_flight' AND lease_owner IS NOT NULL AND lease_expires_at IS NOT NULL)
    OR (status <> 'in_flight' AND lease_owner IS NULL AND lease_expires_at IS NULL)
  ),
  CONSTRAINT chk_webhook_delivery_failure_reason CHECK (
    (status = 'failed' AND failure_reason IS NOT NULL)
    OR (status <> 'failed' AND failure_reason IS NULL)
  ),
  CONSTRAINT chk_webhook_delivery_result CHECK (
    last_result IS NULL OR last_result IN (
      'delivered', 'timeout', 'connect_failed', 'dns_failed', 'http_request_timeout',
      'http_too_many_requests', 'http_server_error', 'http_redirect', 'http_client_error',
      'http_unexpected_status', 'ssrf_blocked', 'config_invalid', 'lease_expired'
    )
  ),
  CONSTRAINT chk_webhook_delivery_failure_reason_value CHECK (
    failure_reason IS NULL OR failure_reason IN ('permanent_error', 'retries_exhausted')
  )
);

CREATE INDEX "idx_webhook_deliveries_due"
  ON "registration_gate_webhook_deliveries" ("next_attempt_at")
  WHERE status IN ('pending', 'retrying');
CREATE INDEX "idx_webhook_deliveries_lease"
  ON "registration_gate_webhook_deliveries" ("lease_expires_at")
  WHERE status = 'in_flight';

CREATE TABLE "registration_gate_webhook_attempts" (
  "event_id" BIGINT NOT NULL
    REFERENCES "registration_gate_webhook_deliveries" ("event_id") ON DELETE CASCADE,
  "attempt_no" INTEGER NOT NULL,
  "started_at" TIMESTAMPTZ NOT NULL,
  "finished_at" TIMESTAMPTZ NOT NULL,
  "result_class" TEXT NOT NULL,
  "http_status" INTEGER,
  PRIMARY KEY ("event_id", "attempt_no")
);
