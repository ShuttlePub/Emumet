CREATE TABLE "accounts" (
  "id" BIGINT PRIMARY KEY,
  "name" TEXT NOT NULL,
  "private_key" TEXT NOT NULL,
  "public_key" TEXT NOT NULL,
  "is_bot" BOOLEAN NOT NULL,
  "deleted_at" TIMESTAMPTZ,
  "suspended_at" TIMESTAMPTZ,
  "suspend_expires_at" TIMESTAMPTZ,
  "suspend_reason" TEXT,
  "banned_at" TIMESTAMPTZ,
  "ban_reason" TEXT,
  "version" BIGINT NOT NULL,
  "nanoid" TEXT UNIQUE NOT NULL,
  "created_at" TIMESTAMPTZ NOT NULL,
  CONSTRAINT chk_suspend_fields
    CHECK ((suspended_at IS NULL AND suspend_reason IS NULL AND suspend_expires_at IS NULL)
        OR (suspended_at IS NOT NULL AND suspend_reason IS NOT NULL)),
  CONSTRAINT chk_ban_fields
    CHECK ((banned_at IS NULL AND ban_reason IS NULL)
        OR (banned_at IS NOT NULL AND ban_reason IS NOT NULL)),
  CONSTRAINT chk_not_both_suspended_and_banned
    CHECK (NOT (suspended_at IS NOT NULL AND banned_at IS NOT NULL))
);

CREATE TABLE "remote_accounts" (
  "id" BIGINT PRIMARY KEY NOT NULL,
  "acct" TEXT UNIQUE NOT NULL,
  "url" TEXT UNIQUE NOT NULL,
  "icon_id" BIGINT
);

CREATE TABLE "profiles" (
  "id" BIGINT PRIMARY KEY NOT NULL,
  "account_id" BIGINT UNIQUE NOT NULL,
  "display" TEXT,
  "summary" TEXT,
  "icon_id" BIGINT,
  "banner_id" BIGINT,
  "version" BIGINT NOT NULL,
  "nanoid" TEXT UNIQUE NOT NULL
);

CREATE TABLE "metadatas" (
  "id" BIGINT PRIMARY KEY NOT NULL,
  "account_id" BIGINT NOT NULL,
  "label" TEXT NOT NULL,
  "content" TEXT NOT NULL,
  "version" BIGINT NOT NULL,
  "nanoid" TEXT UNIQUE NOT NULL
);

CREATE TABLE "auth_hosts" (
  "id" BIGINT PRIMARY KEY NOT NULL,
  "url" TEXT UNIQUE NOT NULL
);

CREATE TABLE "auth_accounts" (
  "id" BIGINT PRIMARY KEY NOT NULL,
  "host_id" BIGINT NOT NULL,
  "client_id" TEXT NOT NULL,
  "version" BIGINT NOT NULL,
  UNIQUE ("host_id", "client_id")
);

CREATE TABLE "auth_emumet_accounts" (
  "emumet_id" BIGINT NOT NULL,
  "auth_id" BIGINT NOT NULL,
  PRIMARY KEY ("emumet_id", "auth_id")
);

CREATE TABLE "follows" (
  "id" BIGINT PRIMARY KEY NOT NULL,
  "follower_local_id" BIGINT,
  "follower_remote_id" BIGINT,
  "followee_local_id" BIGINT,
  "followee_remote_id" BIGINT,
  "approved_at" TIMESTAMPTZ,
  CONSTRAINT chk_follower_exclusive
    CHECK ((follower_local_id IS NOT NULL) != (follower_remote_id IS NOT NULL)),
  CONSTRAINT chk_followee_exclusive
    CHECK ((followee_local_id IS NOT NULL) != (followee_remote_id IS NOT NULL))
);

CREATE TABLE "images" (
  "id" BIGINT PRIMARY KEY NOT NULL,
  "url" TEXT UNIQUE NOT NULL,
  "hash" TEXT NOT NULL,
  "blurhash" TEXT NOT NULL
);

CREATE TABLE "account_events" (
  "version" BIGINT NOT NULL,
  "id" BIGINT NOT NULL,
  "event_name" TEXT NOT NULL,
  "data" JSONB NOT NULL,
  "occurred_at" TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY ("id", "version")
);

CREATE TABLE "auth_account_events" (
  "version" BIGINT NOT NULL,
  "id" BIGINT NOT NULL,
  "event_name" TEXT NOT NULL,
  "data" JSONB NOT NULL,
  "occurred_at" TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY ("id", "version")
);

CREATE TABLE "profile_events" (
  "version" BIGINT NOT NULL,
  "id" BIGINT NOT NULL,
  "event_name" TEXT NOT NULL,
  "data" JSONB NOT NULL,
  "occurred_at" TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY ("id", "version")
);

CREATE TABLE "metadata_events" (
  "version" BIGINT NOT NULL,
  "id" BIGINT NOT NULL,
  "event_name" TEXT NOT NULL,
  "data" JSONB NOT NULL,
  "occurred_at" TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY ("id", "version")
);

CREATE INDEX idx_metadatas_account_id ON metadatas (account_id);

ALTER TABLE "remote_accounts" ADD FOREIGN KEY ("icon_id") REFERENCES "images" ("id") ON DELETE SET NULL;

ALTER TABLE "profiles" ADD FOREIGN KEY ("account_id") REFERENCES "accounts" ("id") ON DELETE CASCADE;

ALTER TABLE "profiles" ADD FOREIGN KEY ("icon_id") REFERENCES "images" ("id") ON DELETE SET NULL;

ALTER TABLE "profiles" ADD FOREIGN KEY ("banner_id") REFERENCES "images" ("id") ON DELETE SET NULL;

ALTER TABLE "metadatas" ADD FOREIGN KEY ("account_id") REFERENCES "accounts" ("id") ON DELETE CASCADE;

ALTER TABLE "auth_accounts" ADD FOREIGN KEY ("host_id") REFERENCES "auth_hosts" ("id") ON DELETE CASCADE;

ALTER TABLE "auth_emumet_accounts" ADD FOREIGN KEY ("auth_id") REFERENCES "auth_accounts" ("id") ON DELETE CASCADE;

ALTER TABLE "auth_emumet_accounts" ADD FOREIGN KEY ("emumet_id") REFERENCES "accounts" ("id") ON DELETE CASCADE;

ALTER TABLE "follows" ADD FOREIGN KEY ("follower_local_id") REFERENCES "accounts" ("id") ON DELETE CASCADE;

ALTER TABLE "follows" ADD FOREIGN KEY ("follower_remote_id") REFERENCES "remote_accounts" ("id") ON DELETE CASCADE;

ALTER TABLE "follows" ADD FOREIGN KEY ("followee_local_id") REFERENCES "accounts" ("id") ON DELETE CASCADE;

ALTER TABLE "follows" ADD FOREIGN KEY ("followee_remote_id") REFERENCES "remote_accounts" ("id") ON DELETE CASCADE;

CREATE TABLE "signing_keys" (
  "id" BIGINT PRIMARY KEY NOT NULL,
  "account_id" BIGINT NOT NULL,
  "algorithm" TEXT NOT NULL,
  "encrypted_private_key" JSONB NOT NULL,
  "public_key_pem" TEXT NOT NULL,
  "key_id_uri" TEXT NOT NULL,
  "created_at" TIMESTAMPTZ NOT NULL DEFAULT now(),
  "revoked_at" TIMESTAMPTZ
);

CREATE INDEX idx_signing_keys_account_id ON signing_keys (account_id);

ALTER TABLE "signing_keys" ADD FOREIGN KEY ("account_id") REFERENCES "accounts" ("id") ON DELETE CASCADE;
