-- Add partial unique indexes to prevent duplicate follows
-- PostgreSQL treats NULLs as distinct in UNIQUE constraints, so we need
-- partial indexes for each follow type combination.

-- Local -> Remote follows (outbound from local user)
CREATE UNIQUE INDEX uq_follows_local_to_remote
    ON follows (follower_local_id, followee_remote_id)
    WHERE follower_local_id IS NOT NULL AND followee_remote_id IS NOT NULL;

-- Remote -> Local follows (inbound from remote user)
CREATE UNIQUE INDEX uq_follows_remote_to_local
    ON follows (follower_remote_id, followee_local_id)
    WHERE follower_remote_id IS NOT NULL AND followee_local_id IS NOT NULL;

-- Local -> Local follows (internal follows)
CREATE UNIQUE INDEX uq_follows_local_to_local
    ON follows (follower_local_id, followee_local_id)
    WHERE follower_local_id IS NOT NULL AND followee_local_id IS NOT NULL;

-- Remote -> Remote follows (should not exist per domain rule, but add for safety)
CREATE UNIQUE INDEX uq_follows_remote_to_remote
    ON follows (follower_remote_id, followee_remote_id)
    WHERE follower_remote_id IS NOT NULL AND followee_remote_id IS NOT NULL;

-- Drop the ineffective constraint from the previous migration
ALTER TABLE follows DROP CONSTRAINT IF EXISTS uq_follows_source_destination;
