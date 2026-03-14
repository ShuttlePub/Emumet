-- SQL dump generated using DBML (dbml.dbdiagram.io)
-- Database: PostgreSQL
-- Generated at: 2025-01-25T10:07:34.134Z

CREATE TABLE "accounts" (
  "id" BIGINT PRIMARY KEY,
  "name" TEXT NOT NULL,
  "private_key" TEXT NOT NULL,
  "public_key" TEXT NOT NULL,
  "is_bot" BOOLEAN NOT NULL,
  "deleted_at" TIMESTAMPTZ,
  "version" BIGINT NOT NULL,
  "nanoid" TEXT UNIQUE NOT NULL,
  "created_at" TIMESTAMPTZ NOT NULL
);

CREATE TABLE "remote_accounts" (
  "id" BIGINT PRIMARY KEY NOT NULL,
  "acct" TEXT UNIQUE NOT NULL,
  "url" TEXT UNIQUE NOT NULL,
  "icon_id" BIGINT
);

CREATE TABLE "profiles" (
  "id" BIGINT PRIMARY KEY NOT NULL,
  "account_id" BIGINT NOT NULL,
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
  "version" BIGINT NOT NULL
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
  "approved_at" TIMESTAMPTZ
);

CREATE TABLE "images" (
  "id" BIGINT PRIMARY KEY NOT NULL,
  "url" TEXT UNIQUE NOT NULL,
  "hash" TEXT NOT NULL,
  "blurhash" TEXT NOT NULL
);

ALTER TABLE "remote_accounts" ADD FOREIGN KEY ("icon_id") REFERENCES "images" ("id") ON DELETE SET NULL;

ALTER TABLE "profiles" ADD FOREIGN KEY ("account_id") REFERENCES "accounts" ("id") ON DELETE CASCADE;

ALTER TABLE "profiles" ADD FOREIGN KEY ("icon_id") REFERENCES "images" ("id") ON DELETE SET NULL;

ALTER TABLE "profiles" ADD FOREIGN KEY ("banner_id") REFERENCES "images" ("id") ON DELETE SET NULL;

ALTER TABLE "metadatas" ADD FOREIGN KEY ("account_id") REFERENCES "accounts" ("id") ON DELETE CASCADE;

ALTER TABLE "auth_accounts" ADD FOREIGN KEY ("host_id") REFERENCES "auth_hosts" ("id") ON DELETE CASCADE;

ALTER TABLE "auth_emumet_accounts" ADD FOREIGN KEY ("auth_id") REFERENCES "auth_accounts" ("id");

ALTER TABLE "auth_emumet_accounts" ADD FOREIGN KEY ("emumet_id") REFERENCES "accounts" ("id") ON DELETE CASCADE;

ALTER TABLE "follows" ADD FOREIGN KEY ("follower_local_id") REFERENCES "accounts" ("id") ON DELETE CASCADE;

ALTER TABLE "follows" ADD FOREIGN KEY ("follower_remote_id") REFERENCES "remote_accounts" ("id") ON DELETE CASCADE;

ALTER TABLE "follows" ADD FOREIGN KEY ("followee_local_id") REFERENCES "accounts" ("id") ON DELETE CASCADE;

ALTER TABLE "follows" ADD FOREIGN KEY ("followee_remote_id") REFERENCES "remote_accounts" ("id") ON DELETE CASCADE;
