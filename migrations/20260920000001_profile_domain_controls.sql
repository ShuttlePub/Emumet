-- Profile x exact-match domain controls (mute / block), D8P.
--
-- One row per (profile, normalized domain). `revision` starts at 1 and increases on every
-- change including delete and re-create: a delete only sets `deleted_at` (tombstone) so the
-- revision counter is never reused and a stale `expected_revision` cannot match a later
-- incarnation. Readers must filter `deleted_at IS NULL`.
CREATE TABLE "profile_domain_controls" (
  "profile_id" BIGINT NOT NULL REFERENCES "profiles" ("id") ON DELETE CASCADE,
  "domain" TEXT NOT NULL,
  "mode" TEXT NOT NULL,
  "revision" BIGINT NOT NULL,
  "created_at" TIMESTAMPTZ NOT NULL DEFAULT now(),
  "updated_at" TIMESTAMPTZ NOT NULL DEFAULT now(),
  "deleted_at" TIMESTAMPTZ,
  PRIMARY KEY ("profile_id", "domain"),
  CONSTRAINT chk_profile_domain_controls_mode CHECK (mode IN ('mute', 'block')),
  CONSTRAINT chk_profile_domain_controls_revision CHECK (revision >= 1),
  CONSTRAINT chk_profile_domain_controls_domain
    CHECK (domain <> '' AND domain = lower(domain) AND domain ~ '^[a-z0-9_.-]+$' AND right(domain, 1) <> '.')
);
