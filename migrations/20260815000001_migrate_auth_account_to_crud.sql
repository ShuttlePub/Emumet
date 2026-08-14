-- AuthAccount ES -> CRUD migration (ADR 0006 Stage 6)
INSERT INTO auth_accounts (id, host_id, client_id)
SELECT DISTINCT ON (aae.id)
    aae.id,
    (aae.data->>'host')::BIGINT,
    aae.data->>'client_id'
FROM auth_account_events aae
LEFT JOIN auth_accounts aa ON aae.id = aa.id
WHERE aa.id IS NULL
ORDER BY aae.id, aae.version;

ALTER TABLE auth_accounts DROP COLUMN version;

DROP TABLE auth_account_events;
