CREATE TABLE "organization_members" (
  "org_account_id" BIGINT NOT NULL,
  "member_account_id" BIGINT NOT NULL,
  "role" TEXT NOT NULL,
  "status" TEXT NOT NULL,
  "invited_by" BIGINT NOT NULL,
  "created_at" TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY ("org_account_id", "member_account_id"),
  CONSTRAINT chk_organization_member_role
    CHECK (role IN ('owner', 'admin', 'member')),
  CONSTRAINT chk_organization_member_status
    CHECK (status IN ('pending', 'active'))
);

CREATE INDEX idx_organization_members_member_account_id
  ON organization_members (member_account_id);

ALTER TABLE "organization_members"
  ADD FOREIGN KEY ("org_account_id") REFERENCES "accounts" ("id") ON DELETE CASCADE;

ALTER TABLE "organization_members"
  ADD FOREIGN KEY ("member_account_id") REFERENCES "accounts" ("id") ON DELETE CASCADE;

ALTER TABLE "organization_members"
  ADD FOREIGN KEY ("invited_by") REFERENCES "accounts" ("id") ON DELETE CASCADE;
