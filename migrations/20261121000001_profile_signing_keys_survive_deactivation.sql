-- Profile actor keys outlive the Profile projection row (P4 review fix).
--
-- Deactivating an Account makes its projector DELETE the Profile row, and reactivating it makes the
-- Profile projector re-materialize the row (same Profile id, rebuilt from the append-only event
-- stream). With ON DELETE CASCADE the Profile's key was deleted by the first step and nothing could
-- bring it back: the actor stayed unresolvable after reactivation (404) and the next restart would
-- have generated a *different* key, i.e. a different identity.
--
-- The key table is keyed by the Profile id, which is stable across deactivation and reactivation, so
-- dropping the foreign key is enough: the retained key re-attaches by itself when the row comes back.
-- The "one active key per Profile" unique index stays. While the row is absent the Profile has no
-- actor (the actor document needs the Profile row), so a retained key is never published.
ALTER TABLE "profile_signing_keys"
  DROP CONSTRAINT IF EXISTS "profile_signing_keys_profile_id_fkey";
