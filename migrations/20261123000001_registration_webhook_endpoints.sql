-- 登録ゲート Webhook の送信先管理 (E15x)。
-- E15w の単一の環境変数送信先を、複数の送信先 (管理者 API で CRUD) へ拡張する。
-- 既存の配送記録・試行履歴は破壊しない: endpoint_id = 0 を「環境変数の送信先」として引き継ぎ、
-- event_id は変えない。環境変数の取り込み (暗号鍵あり) は endpoint_id 0 の記録を取り込んだ送信先へ
-- 付け替える。通知イベントは追記専用なので、ここでも外部キーは張らない。

CREATE TABLE "registration_gate_webhook_endpoints" (
  "id" BIGINT PRIMARY KEY,
  -- 検査・正規化済みの HTTPS URL。送信に必要なので保存するが、管理者 API には部分マスク形でしか返さない。
  "url" TEXT NOT NULL,
  -- 署名 secret は AEAD で暗号化した暗号文のみ保存する (平文は DB に存在しない)。
  -- secret_key_id は暗号化に使った鍵の識別子 (将来の鍵ローテーションに備える)。
  "secret_key_id" TEXT NOT NULL,
  "secret_ciphertext" BYTEA NOT NULL,
  "categories" TEXT[] NOT NULL,
  "revision" BIGINT NOT NULL DEFAULT 1,
  -- この ID より後の通知イベントだけを配送する (作成前のイベントは遡及しない)。
  "baseline_event_id" BIGINT NOT NULL,
  "source" TEXT NOT NULL DEFAULT 'api',
  "created_at" TIMESTAMPTZ NOT NULL,
  "updated_at" TIMESTAMPTZ NOT NULL,
  CONSTRAINT chk_webhook_endpoint_id CHECK ("id" > 0),
  CONSTRAINT chk_webhook_endpoint_categories CHECK (
    cardinality("categories") >= 1
    AND "categories" <@ ARRAY['outage', 'recovery', 'auto_stop', 'override']::TEXT[]
  ),
  CONSTRAINT chk_webhook_endpoint_source CHECK ("source" IN ('api', 'environment_import')),
  CONSTRAINT chk_webhook_endpoint_revision CHECK ("revision" >= 1)
);

-- 正規化後の URL の重複を拒否する (同一送信先への二重配送を防ぐ)。
CREATE UNIQUE INDEX "uq_webhook_endpoints_url" ON "registration_gate_webhook_endpoints" ("url");

-- 送信先管理の監査。URL 全体・secret・通知本文は残さない。送信先の削除後も残るので外部キーは張らない。
CREATE TABLE "registration_gate_webhook_endpoint_audit" (
  "id" BIGINT PRIMARY KEY,
  "actor" BIGINT,
  "endpoint_id" BIGINT,
  "operation" TEXT NOT NULL,
  "outcome" TEXT NOT NULL,
  "revision" BIGINT,
  "detail" TEXT,
  "created_at" TIMESTAMPTZ NOT NULL,
  CONSTRAINT chk_webhook_audit_operation CHECK (
    "operation" IN ('create', 'update', 'delete', 'test', 'retry', 'import')
  ),
  CONSTRAINT chk_webhook_audit_outcome CHECK (
    "outcome" IN ('succeeded', 'rejected', 'revision_conflict', 'not_found', 'delivery_failed')
  )
);
CREATE INDEX "idx_webhook_audit_endpoint" ON "registration_gate_webhook_endpoint_audit" ("endpoint_id", "id");

-- 環境変数の送信先の取り込み済みマーカー。取り込み後に管理者が送信先を削除しても再取り込みしない。
CREATE TABLE "registration_gate_webhook_env_import" (
  "id" SMALLINT PRIMARY KEY CHECK ("id" = 1),
  "endpoint_id" BIGINT NOT NULL,
  "imported_at" TIMESTAMPTZ NOT NULL
);

-- 配送記録を (送信先, event ID) 単位にする。既存の記録は endpoint_id = 0 (環境変数の送信先) になる。
ALTER TABLE "registration_gate_webhook_attempts"
  DROP CONSTRAINT "registration_gate_webhook_attempts_event_id_fkey";
ALTER TABLE "registration_gate_webhook_attempts"
  DROP CONSTRAINT "registration_gate_webhook_attempts_pkey";
ALTER TABLE "registration_gate_webhook_deliveries"
  DROP CONSTRAINT "registration_gate_webhook_deliveries_pkey";

ALTER TABLE "registration_gate_webhook_deliveries"
  ADD COLUMN "endpoint_id" BIGINT NOT NULL DEFAULT 0;
ALTER TABLE "registration_gate_webhook_deliveries"
  ADD COLUMN "failed_at" TIMESTAMPTZ;
ALTER TABLE "registration_gate_webhook_attempts"
  ADD COLUMN "endpoint_id" BIGINT NOT NULL DEFAULT 0;

UPDATE "registration_gate_webhook_deliveries"
  SET "failed_at" = "updated_at" WHERE "status" = 'failed';

ALTER TABLE "registration_gate_webhook_deliveries"
  ALTER COLUMN "endpoint_id" DROP DEFAULT;
ALTER TABLE "registration_gate_webhook_attempts"
  ALTER COLUMN "endpoint_id" DROP DEFAULT;

ALTER TABLE "registration_gate_webhook_deliveries"
  ADD PRIMARY KEY ("endpoint_id", "event_id");
ALTER TABLE "registration_gate_webhook_attempts"
  ADD PRIMARY KEY ("endpoint_id", "event_id", "attempt_no");
ALTER TABLE "registration_gate_webhook_attempts"
  ADD CONSTRAINT "registration_gate_webhook_attempts_delivery_fkey"
  FOREIGN KEY ("endpoint_id", "event_id")
  REFERENCES "registration_gate_webhook_deliveries" ("endpoint_id", "event_id")
  ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE "registration_gate_webhook_deliveries"
  ADD CONSTRAINT chk_webhook_delivery_failed_at CHECK (
    ("status" = 'failed') = ("failed_at" IS NOT NULL)
  );

ALTER TABLE "registration_gate_webhook_deliveries" DROP CONSTRAINT chk_webhook_delivery_result;
ALTER TABLE "registration_gate_webhook_deliveries" ADD CONSTRAINT chk_webhook_delivery_result CHECK (
  last_result IS NULL OR last_result IN (
    'delivered', 'timeout', 'connect_failed', 'dns_failed', 'http_request_timeout',
    'http_too_many_requests', 'http_server_error', 'http_redirect', 'http_client_error',
    'http_unexpected_status', 'ssrf_blocked', 'config_invalid', 'lease_expired',
    'secret_undecryptable'
  )
);

-- failed 一覧の cursor ページング (新しい failed から古い failed へ、全件に到達できる)。
CREATE INDEX "idx_webhook_deliveries_failed"
  ON "registration_gate_webhook_deliveries" ("failed_at" DESC, "endpoint_id" DESC, "event_id" DESC)
  WHERE "status" = 'failed';
CREATE INDEX "idx_webhook_deliveries_endpoint"
  ON "registration_gate_webhook_deliveries" ("endpoint_id", "status");
