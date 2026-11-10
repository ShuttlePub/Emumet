-- P1 profile-actor-identity-foundation (Emumet#71).
-- Profile username storage and Profile-owned actor signing keys.
-- NOTE: the final production migration number is assigned by P4 (profile-federation-in-place-cutover);
-- this file only adds nullable/new objects and does not touch any Account-based federation data.

-- Read-model column. NULL = no username yet (legacy Profiles).
ALTER TABLE "profiles" ADD COLUMN "username" TEXT;
ALTER TABLE "profiles" ADD CONSTRAINT chk_profiles_username_format
  CHECK ("username" IS NULL OR "username" ~ '^[a-z0-9][a-z0-9._-]{0,63}$');
CREATE UNIQUE INDEX profiles_username_unique ON "profiles" ("username") WHERE "username" IS NOT NULL;

-- Source-of-truth uniqueness lives on the append-only event table, which survives Profile (and
-- Account) deletion because projection rows are deleted but events never are:
--   * a username is never reused, not even after the owning Profile was deleted;
--   * a Profile can carry a username event at most once (Created-with-username or UsernameSet),
--     so concurrent or repeated assignments race on the database and exactly one wins.
CREATE UNIQUE INDEX profile_events_username_unique
  ON "profile_events" (("data" ->> 'username'))
  WHERE ("data" ->> 'username') IS NOT NULL;
CREATE UNIQUE INDEX profile_events_username_once_per_profile
  ON "profile_events" ("id")
  WHERE ("data" ->> 'username') IS NOT NULL;

-- Profile actor signing keys. Owner = Profile (not Account), so a Profile transfer
-- (AccountTransferred) does not change key ownership. Rows are removed together with the
-- Profile projection row. No production code path creates rows in P1 (P4 owns the one-time
-- generation).
CREATE TABLE "profile_signing_keys" (
  "id" BIGINT PRIMARY KEY,
  "profile_id" BIGINT NOT NULL REFERENCES "profiles" ("id") ON DELETE CASCADE,
  "algorithm" TEXT NOT NULL,
  "encrypted_private_key" JSONB NOT NULL,
  "public_key_pem" TEXT NOT NULL,
  "key_id_uri" TEXT NOT NULL,
  "created_at" TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  "revoked_at" TIMESTAMPTZ
);
CREATE INDEX idx_profile_signing_keys_profile_id ON "profile_signing_keys" ("profile_id");
CREATE UNIQUE INDEX profile_signing_keys_one_active_per_profile
  ON "profile_signing_keys" ("profile_id") WHERE "revoked_at" IS NULL;
