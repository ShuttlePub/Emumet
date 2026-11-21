-- P1 emumet-instance-identity (K1, Emumet#116).
-- The Emumet instance actor identity and its signing key. Additive: two new tables, no change to
-- any existing table. The instance key is stored apart from the Profile keys (profile_signing_keys)
-- and never shares rows, ids or key id URIs with them.
--
-- An old binary ignores these tables, but it refuses to start once this migration is applied
-- (a recorded migration it does not know); roll back by forward fix, not by dropping the tables.

-- Exactly one identity per deployment (a singleton row): concurrent provisioning races on the
-- primary key and exactly one insert wins. It is bound to the public origin it was created for;
-- the application refuses to run when the configured origin differs.
CREATE TABLE "instance_identity" (
  "singleton" BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK ("singleton"),
  "public_origin" TEXT NOT NULL
    CHECK ("public_origin" ~ '^https?://[a-z0-9.:\[\]-]+$' AND "public_origin" = lower("public_origin")),
  "created_at" TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Instance signing keys. "id" is the opaque, random part of the key id; "key_id_uri" is
-- `{origin}/ap/instance#key-{id}` and is unique for ever, so a key id is never reused (rows are
-- never deleted, see the triggers below). "revision" is the cache coherence token of the in-process
-- key cache: K1 never changes it, K2 bumps it when it changes the state of a key.
--
-- Signing and publication are separate states of a key:
--   active  (retired_at IS NULL AND revoked_at IS NULL): signs; published. At most one (index below).
--   retired (retired_at set, revoked_at NULL):           no longer signs; still published, so that
--                                                        signatures made earlier stay verifiable.
--   revoked (revoked_at set):                            neither signs nor is published.
-- K1 only ever creates the one active key; moving a key to retired / revoked is K2.
CREATE TABLE "instance_signing_keys" (
  "id" TEXT PRIMARY KEY CHECK ("id" ~ '^[0-9a-f]{32}$'),
  "singleton" BOOLEAN NOT NULL DEFAULT TRUE REFERENCES "instance_identity" ("singleton"),
  "revision" BIGINT NOT NULL DEFAULT 1 CHECK ("revision" >= 1),
  "algorithm" TEXT NOT NULL,
  "encrypted_private_key" JSONB NOT NULL,
  "public_key_pem" TEXT NOT NULL,
  "key_id_uri" TEXT NOT NULL UNIQUE,
  "created_at" TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  "retired_at" TIMESTAMPTZ,
  "revoked_at" TIMESTAMPTZ
);
-- At most one active (signing) key for the one identity. Retired and revoked keys are not limited.
CREATE UNIQUE INDEX instance_signing_keys_one_active
  ON "instance_signing_keys" ("singleton") WHERE "retired_at" IS NULL AND "revoked_at" IS NULL;

-- Identity and keys are never deleted or truncated, and the identity and the key material never
-- change: replacing a damaged or mismatched key by silently generating another one is not possible
-- through any code path (rotation / revocation is K2). Only retired_at / revoked_at / revision of a
-- key may change.
CREATE FUNCTION instance_identity_reject_change() RETURNS trigger AS $$
BEGIN
  RAISE EXCEPTION 'the instance identity and its keys are never deleted or replaced (%)', TG_OP
    USING ERRCODE = 'restrict_violation';
END;
$$ LANGUAGE plpgsql;

CREATE FUNCTION instance_signing_keys_reject_material_change() RETURNS trigger AS $$
BEGIN
  IF NEW."id" IS DISTINCT FROM OLD."id"
     OR NEW."singleton" IS DISTINCT FROM OLD."singleton"
     OR NEW."algorithm" IS DISTINCT FROM OLD."algorithm"
     OR NEW."encrypted_private_key" IS DISTINCT FROM OLD."encrypted_private_key"
     OR NEW."public_key_pem" IS DISTINCT FROM OLD."public_key_pem"
     OR NEW."key_id_uri" IS DISTINCT FROM OLD."key_id_uri"
     OR NEW."created_at" IS DISTINCT FROM OLD."created_at" THEN
    RAISE EXCEPTION 'instance signing key material and identifiers are immutable'
      USING ERRCODE = 'restrict_violation';
  END IF;
  RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER instance_identity_immutable
  BEFORE UPDATE OR DELETE ON "instance_identity"
  FOR EACH ROW EXECUTE FUNCTION instance_identity_reject_change();
CREATE TRIGGER instance_identity_no_truncate
  BEFORE TRUNCATE ON "instance_identity"
  FOR EACH STATEMENT EXECUTE FUNCTION instance_identity_reject_change();
CREATE TRIGGER instance_signing_keys_no_delete
  BEFORE DELETE ON "instance_signing_keys"
  FOR EACH ROW EXECUTE FUNCTION instance_identity_reject_change();
CREATE TRIGGER instance_signing_keys_no_truncate
  BEFORE TRUNCATE ON "instance_signing_keys"
  FOR EACH STATEMENT EXECUTE FUNCTION instance_identity_reject_change();
CREATE TRIGGER instance_signing_keys_immutable_material
  BEFORE UPDATE ON "instance_signing_keys"
  FOR EACH ROW EXECUTE FUNCTION instance_signing_keys_reject_material_change();
