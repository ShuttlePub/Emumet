-- Profile 単位の social graph の保存スキーマ (PF / P2)。
--
-- これは **隔離 DB 用の fixture** で、本番の migrations/ には配置しない。本番 migration の採番と配置は
-- P4 (profile-federation-in-place-cutover) が所有する。適用の前提は、本番 migrations/ を全て適用した DB
-- (profiles テーブルが存在すること)。旧 Account 単位のテーブルには触れない。
--
-- URL 系の列 (actor_url / activity_id / target_inbox) は 1024 バイト以下に制限する (kernel の検証と同じ上限)。
-- 複合 B-tree 索引 (URL 2 列 + 数値列) の 1 エントリが PostgreSQL の上限 (約 2.7KB) を超えないようにするため。
--
-- 設計:
--   * 関係の主体はローカル Profile (profiles.id)。リモート側は canonical actor URL だけを持つ。
--   * ローカル側の外部キーは profiles への ON DELETE CASCADE のみ。旧テーブル (remote_accounts 等) を
--     参照しないので、旧連合データの初期化 (02_reset_legacy_federation.sql) と独立している。

CREATE TABLE "profile_follows" (
  "id" BIGINT PRIMARY KEY,
  "nanoid" TEXT NOT NULL UNIQUE,
  "follower_profile_id" BIGINT REFERENCES "profiles" ("id") ON DELETE CASCADE,
  "follower_actor_url" TEXT,
  "followee_profile_id" BIGINT REFERENCES "profiles" ("id") ON DELETE CASCADE,
  "followee_actor_url" TEXT,
  "activity_id" TEXT NOT NULL,
  "state" TEXT NOT NULL,
  "end_reason" TEXT,
  "accepted_at" TIMESTAMPTZ,
  "ended_at" TIMESTAMPTZ,
  "created_at" TIMESTAMPTZ NOT NULL,
  "updated_at" TIMESTAMPTZ NOT NULL,
  CONSTRAINT chk_profile_follows_url_length
    CHECK (octet_length(follower_actor_url) <= 1024 AND octet_length(followee_actor_url) <= 1024
       AND octet_length(activity_id) <= 1024),
  CONSTRAINT chk_profile_follows_follower_exclusive
    CHECK ((follower_profile_id IS NOT NULL) <> (follower_actor_url IS NOT NULL)),
  CONSTRAINT chk_profile_follows_followee_exclusive
    CHECK ((followee_profile_id IS NOT NULL) <> (followee_actor_url IS NOT NULL)),
  CONSTRAINT chk_profile_follows_not_remote_to_remote
    CHECK (follower_profile_id IS NOT NULL OR followee_profile_id IS NOT NULL),
  CONSTRAINT chk_profile_follows_not_self
    CHECK (follower_profile_id IS DISTINCT FROM followee_profile_id),
  CONSTRAINT chk_profile_follows_state
    CHECK (state IN ('pending', 'accepted', 'ended')),
  CONSTRAINT chk_profile_follows_end_reason
    CHECK (end_reason IS NULL OR end_reason IN ('undone', 'rejected', 'blocked')),
  CONSTRAINT chk_profile_follows_ended
    CHECK ((state = 'ended' AND ended_at IS NOT NULL AND end_reason IS NOT NULL)
        OR (state <> 'ended' AND ended_at IS NULL AND end_reason IS NULL)),
  CONSTRAINT chk_profile_follows_accepted
    CHECK ((state = 'accepted' AND accepted_at IS NOT NULL)
        OR (state = 'pending' AND accepted_at IS NULL)
        OR state = 'ended')
);

-- 有効な世代 (pending / accepted) は (follower, followee) の組ごとに高々 1 つ。
-- NULL は一意性上 distinct 扱いなので、local/remote の組合せごとに partial unique index を張る。
CREATE UNIQUE INDEX uq_profile_follows_active_local_local
  ON profile_follows (follower_profile_id, followee_profile_id)
  WHERE state <> 'ended' AND follower_profile_id IS NOT NULL AND followee_profile_id IS NOT NULL;
CREATE UNIQUE INDEX uq_profile_follows_active_local_remote
  ON profile_follows (follower_profile_id, followee_actor_url)
  WHERE state <> 'ended' AND follower_profile_id IS NOT NULL AND followee_actor_url IS NOT NULL;
CREATE UNIQUE INDEX uq_profile_follows_active_remote_local
  ON profile_follows (follower_actor_url, followee_profile_id)
  WHERE state <> 'ended' AND follower_actor_url IS NOT NULL AND followee_profile_id IS NOT NULL;

-- 世代の識別子 = follower が発行した元 Follow activity ID。終了済みも残すので、古い activity の再送で
-- 世代が復活することはない。activity ID は follower ごとに名前空間を分ける。
CREATE UNIQUE INDEX uq_profile_follows_generation_local
  ON profile_follows (follower_profile_id, activity_id)
  WHERE follower_profile_id IS NOT NULL;
CREATE UNIQUE INDEX uq_profile_follows_generation_remote
  ON profile_follows (follower_actor_url, activity_id)
  WHERE follower_actor_url IS NOT NULL;

CREATE INDEX idx_profile_follows_following
  ON profile_follows (follower_profile_id, id) WHERE state <> 'ended';
CREATE INDEX idx_profile_follows_followers
  ON profile_follows (followee_profile_id, id) WHERE state <> 'ended';

CREATE TABLE "profile_blocks" (
  "id" BIGINT PRIMARY KEY,
  "nanoid" TEXT NOT NULL UNIQUE,
  "owner_profile_id" BIGINT NOT NULL REFERENCES "profiles" ("id") ON DELETE CASCADE,
  "target_profile_id" BIGINT REFERENCES "profiles" ("id") ON DELETE CASCADE,
  "target_actor_url" TEXT,
  "created_at" TIMESTAMPTZ NOT NULL,
  CONSTRAINT chk_profile_blocks_url_length CHECK (octet_length(target_actor_url) <= 1024),
  CONSTRAINT chk_profile_blocks_target_exclusive
    CHECK ((target_profile_id IS NOT NULL) <> (target_actor_url IS NOT NULL)),
  CONSTRAINT chk_profile_blocks_not_self
    CHECK (target_profile_id IS DISTINCT FROM owner_profile_id)
);
CREATE UNIQUE INDEX uq_profile_blocks_local
  ON profile_blocks (owner_profile_id, target_profile_id) WHERE target_profile_id IS NOT NULL;
CREATE UNIQUE INDEX uq_profile_blocks_remote
  ON profile_blocks (owner_profile_id, target_actor_url) WHERE target_actor_url IS NOT NULL;

CREATE TABLE "profile_mutes" (
  "id" BIGINT PRIMARY KEY,
  "nanoid" TEXT NOT NULL UNIQUE,
  "owner_profile_id" BIGINT NOT NULL REFERENCES "profiles" ("id") ON DELETE CASCADE,
  "target_profile_id" BIGINT REFERENCES "profiles" ("id") ON DELETE CASCADE,
  "target_actor_url" TEXT,
  "created_at" TIMESTAMPTZ NOT NULL,
  CONSTRAINT chk_profile_mutes_url_length CHECK (octet_length(target_actor_url) <= 1024),
  CONSTRAINT chk_profile_mutes_target_exclusive
    CHECK ((target_profile_id IS NOT NULL) <> (target_actor_url IS NOT NULL)),
  CONSTRAINT chk_profile_mutes_not_self
    CHECK (target_profile_id IS DISTINCT FROM owner_profile_id)
);
CREATE UNIQUE INDEX uq_profile_mutes_local
  ON profile_mutes (owner_profile_id, target_profile_id) WHERE target_profile_id IS NOT NULL;
CREATE UNIQUE INDEX uq_profile_mutes_remote
  ON profile_mutes (owner_profile_id, target_actor_url) WHERE target_actor_url IS NOT NULL;

-- 受信 activity の重複検知記録。キーは (受信 Profile, actor, activity ID)。
CREATE TABLE "profile_inbound_activities" (
  "receiver_profile_id" BIGINT NOT NULL REFERENCES "profiles" ("id") ON DELETE CASCADE,
  "actor_url" TEXT NOT NULL,
  "activity_id" TEXT NOT NULL,
  "activity_type" TEXT NOT NULL,
  "receive_count" INTEGER NOT NULL DEFAULT 1 CHECK ("receive_count" > 0),
  "first_received_at" TIMESTAMPTZ NOT NULL,
  "last_received_at" TIMESTAMPTZ NOT NULL,
  PRIMARY KEY ("receiver_profile_id", "actor_url", "activity_id"),
  CONSTRAINT chk_profile_inbound_activities_url_length
    CHECK (octet_length(actor_url) <= 1024 AND octet_length(activity_id) <= 1024)
);
CREATE INDEX idx_profile_inbound_activities_first_received
  ON profile_inbound_activities (first_received_at);

-- 配送 job。payload は送信する本文のバイト列そのもの (HTTP 署名 digest のため正規化しない TEXT)。
CREATE TABLE "profile_delivery_jobs" (
  "id" BIGINT PRIMARY KEY,
  "sender_profile_id" BIGINT NOT NULL REFERENCES "profiles" ("id") ON DELETE CASCADE,
  "activity_type" TEXT NOT NULL,
  "activity_id" TEXT NOT NULL,
  "target_inbox" TEXT NOT NULL,
  "payload" TEXT NOT NULL,
  "follow_id" BIGINT REFERENCES "profile_follows" ("id") ON DELETE SET NULL,
  "state" TEXT NOT NULL,
  "attempt_count" INTEGER NOT NULL DEFAULT 0,
  "next_attempt_at" TIMESTAMPTZ NOT NULL,
  "last_attempt_at" TIMESTAMPTZ,
  "last_result" TEXT,
  "last_http_status" INTEGER,
  "failure_reason" TEXT,
  "lease_owner" TEXT,
  "lease_token" TEXT,
  "lease_expires_at" TIMESTAMPTZ,
  "delivered_at" TIMESTAMPTZ,
  "cancelled_at" TIMESTAMPTZ,
  "cancel_reason" TEXT,
  "created_at" TIMESTAMPTZ NOT NULL,
  "updated_at" TIMESTAMPTZ NOT NULL,
  CONSTRAINT chk_profile_delivery_jobs_url_length
    CHECK (octet_length(activity_id) <= 1024 AND octet_length(target_inbox) <= 1024),
  CONSTRAINT uq_profile_delivery_jobs_key UNIQUE ("sender_profile_id", "activity_id", "target_inbox"),
  CONSTRAINT chk_profile_delivery_jobs_activity_type
    CHECK (activity_type IN ('Follow', 'Accept', 'Reject', 'Undo')),
  CONSTRAINT chk_profile_delivery_jobs_state
    CHECK (state IN ('pending', 'in_flight', 'retrying', 'delivered', 'failed', 'cancelled')),
  CONSTRAINT chk_profile_delivery_jobs_lease
    CHECK ((state = 'in_flight'
              AND lease_owner IS NOT NULL AND lease_token IS NOT NULL AND lease_expires_at IS NOT NULL)
        OR (state <> 'in_flight'
              AND lease_owner IS NULL AND lease_token IS NULL AND lease_expires_at IS NULL)),
  CONSTRAINT chk_profile_delivery_jobs_failure
    CHECK ((state = 'failed' AND failure_reason IS NOT NULL)
        OR (state <> 'failed' AND failure_reason IS NULL)),
  CONSTRAINT chk_profile_delivery_jobs_delivered
    CHECK ((state = 'delivered') = (delivered_at IS NOT NULL)),
  CONSTRAINT chk_profile_delivery_jobs_cancelled
    CHECK ((state = 'cancelled') = (cancelled_at IS NOT NULL))
);
CREATE INDEX idx_profile_delivery_jobs_due
  ON profile_delivery_jobs (next_attempt_at, id) WHERE state IN ('pending', 'retrying');
CREATE INDEX idx_profile_delivery_jobs_lease
  ON profile_delivery_jobs (lease_expires_at) WHERE state = 'in_flight';
CREATE INDEX idx_profile_delivery_jobs_follow
  ON profile_delivery_jobs (follow_id) WHERE follow_id IS NOT NULL;

CREATE TABLE "profile_delivery_attempts" (
  "job_id" BIGINT NOT NULL REFERENCES "profile_delivery_jobs" ("id") ON DELETE CASCADE,
  "attempt_no" INTEGER NOT NULL,
  "started_at" TIMESTAMPTZ NOT NULL,
  "finished_at" TIMESTAMPTZ NOT NULL,
  "result_class" TEXT NOT NULL,
  "http_status" INTEGER,
  PRIMARY KEY ("job_id", "attempt_no")
);
