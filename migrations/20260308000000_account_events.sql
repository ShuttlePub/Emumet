CREATE TABLE "account_events" (
  "version" UUID NOT NULL,
  "id" UUID NOT NULL,
  "event_name" TEXT NOT NULL,
  "data" JSON NOT NULL,
  PRIMARY KEY ("id", "version")
);
