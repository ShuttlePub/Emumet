-- Durable ledger: intentionally NO FK to profiles/accounts, which are projections.
CREATE TABLE profile_links (
    id text PRIMARY KEY CHECK (length(id) BETWEEN 16 AND 128),
    profile_id bigint NOT NULL,
    owner_account_id bigint NOT NULL,
    registered_account_version bigint NOT NULL,
    actor_snapshot text NOT NULL,
    origin text NOT NULL,
    trust_mode smallint NOT NULL CHECK (trust_mode = 3),
    generation bigint NOT NULL DEFAULT 1 CHECK (generation > 0),
    state text NOT NULL CHECK (state IN ('active', 'unlinked')),
    created_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    unlinked_at timestamptz,
    unlink_reason text,
    deactivated_account_id bigint,
    deactivated_account_version bigint,
    CHECK ((state = 'active' AND unlinked_at IS NULL AND unlink_reason IS NULL)
        OR (state = 'unlinked' AND unlinked_at IS NOT NULL AND unlink_reason IS NOT NULL))
);
CREATE UNIQUE INDEX profile_links_one_active_home ON profile_links(profile_id)
    WHERE state = 'active' AND trust_mode = 3;
CREATE INDEX profile_links_owner ON profile_links(owner_account_id, profile_id, id);

-- Physical deletion is only a defensive path, never the trigger for normal deactivation.
-- An uncoordinated deletion must fail rather than silently erase an active link.
CREATE FUNCTION guard_profile_link_delete() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM profile_links WHERE profile_id = OLD.id AND state = 'active') THEN
        RAISE EXCEPTION 'active home link must be revoked before profile deletion' USING ERRCODE = '23514';
    END IF;
    RETURN OLD;
END $$;
CREATE TRIGGER profile_link_delete_guard BEFORE DELETE ON profiles
    FOR EACH ROW EXECUTE FUNCTION guard_profile_link_delete();
