CREATE TABLE "signing_keys" (
  "id" BIGINT PRIMARY KEY,
  "account_id" BIGINT NOT NULL REFERENCES "accounts" ("id"),
  "algorithm" TEXT NOT NULL,
  "encrypted_private_key" JSONB NOT NULL,
  "public_key_pem" TEXT NOT NULL,
  "key_id_uri" TEXT NOT NULL,
  "created_at" TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  "revoked_at" TIMESTAMPTZ
);

CREATE INDEX idx_signing_keys_account_id ON signing_keys (account_id);
