ALTER TABLE "accounts"
  ADD COLUMN "kind" TEXT NOT NULL DEFAULT 'personal';

ALTER TABLE "accounts"
  ADD CONSTRAINT chk_account_kind
  CHECK (kind IN ('personal', 'organization'));
