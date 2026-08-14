-- AuthAccount ES -> CRUD migration (ADR 0006 Stage 6)
-- The version column is no longer part of the AuthAccount entity. Drop it
-- before backfilling so that missing rows can be inserted without supplying a
-- value for the removed column.
ALTER TABLE auth_accounts DROP COLUMN IF EXISTS version;

-- Fold existing auth_account_events into auth_accounts.
INSERT INTO auth_accounts (id, host_id, client_id)
SELECT DISTINCT ON (aae.id)
    aae.id,
    (aae.data->>'host')::BIGINT,
    aae.data->>'client_id'
FROM auth_account_events aae
LEFT JOIN auth_accounts aa ON aae.id = aa.id
WHERE aa.id IS NULL
ORDER BY aae.id, aae.version;

DROP TABLE IF EXISTS auth_account_events;
