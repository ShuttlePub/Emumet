-- D08 acceptance must: include Profiles hidden by pre-cutover Account deactivation.
-- Intentionally ordered BEFORE the first P4 migration (20261120000001): do not change the
-- checksum of a published migration, and refuse before any P4 schema/reset is applied.
-- sqlx also applies this previously absent version on databases that already ran P4. In that
-- case the reset ledger skips the cutover-only prerequisite; nothing is reset or rewritten.
-- Profile events have no Deleted variant: every stream is an existing, restorable Profile.
-- username is assigned once, in Created or UsernameSet, and survives projection deletion.
DO $precheck$
DECLARE
  already_reset BOOLEAN := false;
  missing_username BIGINT;
BEGIN
  IF to_regclass('public.profile_federation_resets') IS NOT NULL THEN
    EXECUTE 'SELECT EXISTS (SELECT 1 FROM profile_federation_resets WHERE reset_id = ''legacy-account-federation-v1'')'
      INTO already_reset;
  END IF;
  IF NOT already_reset THEN
    WITH existing_profiles AS (
      SELECT id FROM profiles
      UNION
      SELECT id FROM profile_events
    )
    SELECT count(*) INTO missing_username
    FROM existing_profiles p
    WHERE NOT EXISTS (SELECT 1 FROM profiles r WHERE r.id = p.id AND r.username IS NOT NULL)
      AND NOT EXISTS (
        SELECT 1 FROM profile_events e
        WHERE e.id = p.id AND e.data ->> 'username' IS NOT NULL
      );
    IF missing_username > 0 THEN
      RAISE EXCEPTION
        'profile federation cutover refused: % profile(s) have no username (including deactivated/unprojected Profiles). With the pre-cutover server, reactivate their owner Accounts, wait for projection, set usernames, and start again. Nothing was changed.',
        missing_username
        USING ERRCODE = 'check_violation';
    END IF;
  END IF;
END
$precheck$;
