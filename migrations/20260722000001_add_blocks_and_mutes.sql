-- Add blocks and mutes tables for user block/mute relationships.
-- Shape mirrors the follows table: exactly one of local/remote per side,
-- enforced by CHECK constraints, with partial unique indexes per combination
-- (PostgreSQL treats NULLs as distinct in UNIQUE constraints).

CREATE TABLE "blocks" (
  "id" BIGINT PRIMARY KEY NOT NULL,
  "blocker_local_id" BIGINT,
  "blocker_remote_id" BIGINT,
  "blocked_local_id" BIGINT,
  "blocked_remote_id" BIGINT,
  CONSTRAINT chk_blocker_exclusive
    CHECK ((blocker_local_id IS NOT NULL) != (blocker_remote_id IS NOT NULL)),
  CONSTRAINT chk_blocked_exclusive
    CHECK ((blocked_local_id IS NOT NULL) != (blocked_remote_id IS NOT NULL))
);

ALTER TABLE "blocks" ADD FOREIGN KEY ("blocker_local_id") REFERENCES "accounts" ("id") ON DELETE CASCADE;
ALTER TABLE "blocks" ADD FOREIGN KEY ("blocker_remote_id") REFERENCES "remote_accounts" ("id") ON DELETE CASCADE;
ALTER TABLE "blocks" ADD FOREIGN KEY ("blocked_local_id") REFERENCES "accounts" ("id") ON DELETE CASCADE;
ALTER TABLE "blocks" ADD FOREIGN KEY ("blocked_remote_id") REFERENCES "remote_accounts" ("id") ON DELETE CASCADE;

-- Local -> Remote blocks
CREATE UNIQUE INDEX uq_blocks_local_to_remote
    ON blocks (blocker_local_id, blocked_remote_id)
    WHERE blocker_local_id IS NOT NULL AND blocked_remote_id IS NOT NULL;

-- Remote -> Local blocks
CREATE UNIQUE INDEX uq_blocks_remote_to_local
    ON blocks (blocker_remote_id, blocked_local_id)
    WHERE blocker_remote_id IS NOT NULL AND blocked_local_id IS NOT NULL;

-- Local -> Local blocks
CREATE UNIQUE INDEX uq_blocks_local_to_local
    ON blocks (blocker_local_id, blocked_local_id)
    WHERE blocker_local_id IS NOT NULL AND blocked_local_id IS NOT NULL;

-- Remote -> Remote blocks (should not exist per domain rule, but add for safety)
CREATE UNIQUE INDEX uq_blocks_remote_to_remote
    ON blocks (blocker_remote_id, blocked_remote_id)
    WHERE blocker_remote_id IS NOT NULL AND blocked_remote_id IS NOT NULL;

CREATE TABLE "mutes" (
  "id" BIGINT PRIMARY KEY NOT NULL,
  "muter_local_id" BIGINT,
  "muter_remote_id" BIGINT,
  "muted_local_id" BIGINT,
  "muted_remote_id" BIGINT,
  CONSTRAINT chk_muter_exclusive
    CHECK ((muter_local_id IS NOT NULL) != (muter_remote_id IS NOT NULL)),
  CONSTRAINT chk_muted_exclusive
    CHECK ((muted_local_id IS NOT NULL) != (muted_remote_id IS NOT NULL))
);

ALTER TABLE "mutes" ADD FOREIGN KEY ("muter_local_id") REFERENCES "accounts" ("id") ON DELETE CASCADE;
ALTER TABLE "mutes" ADD FOREIGN KEY ("muter_remote_id") REFERENCES "remote_accounts" ("id") ON DELETE CASCADE;
ALTER TABLE "mutes" ADD FOREIGN KEY ("muted_local_id") REFERENCES "accounts" ("id") ON DELETE CASCADE;
ALTER TABLE "mutes" ADD FOREIGN KEY ("muted_remote_id") REFERENCES "remote_accounts" ("id") ON DELETE CASCADE;

-- Local -> Remote mutes
CREATE UNIQUE INDEX uq_mutes_local_to_remote
    ON mutes (muter_local_id, muted_remote_id)
    WHERE muter_local_id IS NOT NULL AND muted_remote_id IS NOT NULL;

-- Remote -> Local mutes
CREATE UNIQUE INDEX uq_mutes_remote_to_local
    ON mutes (muter_remote_id, muted_local_id)
    WHERE muter_remote_id IS NOT NULL AND muted_local_id IS NOT NULL;

-- Local -> Local mutes
CREATE UNIQUE INDEX uq_mutes_local_to_local
    ON mutes (muter_local_id, muted_local_id)
    WHERE muter_local_id IS NOT NULL AND muted_local_id IS NOT NULL;

-- Remote -> Remote mutes (should not exist per domain rule, but add for safety)
CREATE UNIQUE INDEX uq_mutes_remote_to_remote
    ON mutes (muter_remote_id, muted_remote_id)
    WHERE muter_remote_id IS NOT NULL AND muted_remote_id IS NOT NULL;
