-- SQL dump generated using DBML (dbml.dbdiagram.io)
-- Database: PostgreSQL
-- Generated at: 2024-09-23T15:01:17.449Z

CREATE TABLE "event_streams" (
  "version" UUID NOT NULL,
  "id" UUID NOT NULL,
  "event_name" TEXT NOT NULL,
  "data" JSON NOT NULL,
  "created_at" TIMESTAMPTZ NOT NULL,
  PRIMARY KEY ("id", "version")
);

CREATE TABLE "accounts" (
  "id" UUID PRIMARY KEY,
  "name" TEXT NOT NULL,
  "private_key" TEXT NOT NULL,
  "public_key" TEXT NOT NULL,
  "is_bot" BOOLEAN NOT NULL,
  "created_at" TIMESTAMPTZ NOT NULL,
  "deleted_at" TIMESTAMPTZ
);

CREATE TABLE "remote_accounts" (
  "id" UUID PRIMARY KEY NOT NULL,
  "acct" TEXT UNIQUE NOT NULL,
  "url" TEXT UNIQUE NOT NULL,
  "icon_id" UUID
);

CREATE TABLE "profiles" (
  "id" UUID PRIMARY KEY NOT NULL,
  "account_id" UUID NOT NULL,
  "display" TEXT,
  "summary" TEXT,
  "icon_id" UUID,
  "banner_id" UUID
);

CREATE TABLE "metadatas" (
  "id" UUID PRIMARY KEY NOT NULL,
  "account_id" UUID NOT NULL,
  "label" TEXT NOT NULL,
  "content" TEXT NOT NULL,
  "created_at" TIMESTAMPTZ NOT NULL
);

CREATE TABLE "stellar_hosts" (
  "id" UUID PRIMARY KEY NOT NULL,
  "url" TEXT UNIQUE NOT NULL
);

CREATE TABLE "stellar_accounts" (
  "id" UUID PRIMARY KEY NOT NULL,
  "host_id" UUID NOT NULL,
  "client_id" TEXT NOT NULL,
  "access_token" TEXT NOT NULL,
  "refresh_token" TEXT NOT NULL
);

CREATE TABLE "stellar_emumet_accounts" (
  "emumet_id" UUID NOT NULL,
  "stellar_id" UUID NOT NULL,
  PRIMARY KEY ("emumet_id", "stellar_id")
);

CREATE TABLE "follows" (
  "id" UUID PRIMARY KEY NOT NULL,
  "follower_local_id" UUID,
  "follower_remote_id" UUID,
  "followee_local_id" UUID,
  "followee_remote_id" UUID,
  "approved_at" TIMESTAMPTZ
);

CREATE TABLE "images" (
  "id" UUID PRIMARY KEY NOT NULL,
  "url" TEXT UNIQUE NOT NULL,
  "hash" TEXT NOT NULL,
  "blurhash" TEXT NOT NULL
);

ALTER TABLE "remote_accounts" ADD FOREIGN KEY ("icon_id") REFERENCES "images" ("id") ON DELETE SET NULL;

ALTER TABLE "profiles" ADD FOREIGN KEY ("account_id") REFERENCES "accounts" ("id") ON DELETE CASCADE;

ALTER TABLE "profiles" ADD FOREIGN KEY ("icon_id") REFERENCES "images" ("id") ON DELETE SET NULL;

ALTER TABLE "profiles" ADD FOREIGN KEY ("banner_id") REFERENCES "images" ("id") ON DELETE SET NULL;

ALTER TABLE "metadatas" ADD FOREIGN KEY ("account_id") REFERENCES "accounts" ("id") ON DELETE CASCADE;

ALTER TABLE "stellar_accounts" ADD FOREIGN KEY ("host_id") REFERENCES "stellar_hosts" ("id") ON DELETE CASCADE;

ALTER TABLE "stellar_emumet_accounts" ADD FOREIGN KEY ("stellar_id") REFERENCES "stellar_accounts" ("id");

ALTER TABLE "stellar_emumet_accounts" ADD FOREIGN KEY ("emumet_id") REFERENCES "accounts" ("id") ON DELETE CASCADE;

ALTER TABLE "follows" ADD FOREIGN KEY ("follower_local_id") REFERENCES "accounts" ("id") ON DELETE CASCADE;

ALTER TABLE "follows" ADD FOREIGN KEY ("follower_remote_id") REFERENCES "remote_accounts" ("id") ON DELETE CASCADE;

ALTER TABLE "follows" ADD FOREIGN KEY ("followee_local_id") REFERENCES "accounts" ("id") ON DELETE CASCADE;

ALTER TABLE "follows" ADD FOREIGN KEY ("followee_remote_id") REFERENCES "remote_accounts" ("id") ON DELETE CASCADE;
