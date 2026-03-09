CREATE TABLE "metadata_events" (
  "version" UUID NOT NULL,
  "id" UUID NOT NULL,
  "event_name" TEXT NOT NULL,
  "data" JSONB NOT NULL,
  "occurred_at" TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY ("id", "version")
);
