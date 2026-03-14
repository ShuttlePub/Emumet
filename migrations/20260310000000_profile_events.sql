CREATE TABLE "profile_events" (
  "version" BIGINT NOT NULL,
  "id" BIGINT NOT NULL,
  "event_name" TEXT NOT NULL,
  "data" JSONB NOT NULL,
  "occurred_at" TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY ("id", "version")
);

ALTER TABLE "profiles" ADD CONSTRAINT "profiles_account_id_unique" UNIQUE ("account_id");
