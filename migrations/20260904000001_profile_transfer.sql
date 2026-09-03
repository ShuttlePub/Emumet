CREATE TABLE "profile_transfer_request_events" (
  "id" BIGINT NOT NULL,
  "version" BIGINT NOT NULL,
  "event_name" TEXT NOT NULL,
  "data" JSONB NOT NULL,
  "occurred_at" TIMESTAMPTZ NOT NULL DEFAULT now(),
  "seq" BIGSERIAL,
  PRIMARY KEY ("id", "version")
);
CREATE INDEX idx_profile_transfer_request_events_seq ON profile_transfer_request_events (seq);

CREATE TABLE "profile_transfer_requests" (
  "id" BIGINT PRIMARY KEY,
  "profile_id" BIGINT NOT NULL REFERENCES "profiles" ("id") ON DELETE CASCADE,
  "from_account_id" BIGINT NOT NULL REFERENCES "accounts" ("id") ON DELETE CASCADE,
  "to_org_account_id" BIGINT NOT NULL REFERENCES "accounts" ("id") ON DELETE CASCADE,
  "status" TEXT NOT NULL DEFAULT 'pending',
  "version" BIGINT NOT NULL,
  "nanoid" TEXT UNIQUE NOT NULL,
  "created_at" TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT chk_profile_transfer_request_status
    CHECK (status IN ('pending', 'accepted', 'rejected', 'cancelled'))
);
CREATE INDEX idx_profile_transfer_requests_profile_id ON profile_transfer_requests (profile_id);
CREATE INDEX idx_profile_transfer_requests_to_org ON profile_transfer_requests (to_org_account_id);
CREATE UNIQUE INDEX profile_transfer_requests_one_pending_per_profile
  ON profile_transfer_requests (profile_id) WHERE status = 'pending';

ALTER TABLE "profiles" ADD COLUMN "owner_kind" TEXT NOT NULL DEFAULT 'personal';
ALTER TABLE "profiles" ADD CONSTRAINT chk_profiles_owner_kind
  CHECK (owner_kind IN ('personal', 'organization'));
UPDATE "profiles" SET "owner_kind" =
  (SELECT "kind" FROM "accounts" WHERE "accounts"."id" = "profiles"."account_id");
ALTER TABLE "profiles" DROP CONSTRAINT "profiles_account_id_key";
CREATE UNIQUE INDEX profiles_personal_account_unique
  ON "profiles" ("account_id") WHERE owner_kind = 'personal';
