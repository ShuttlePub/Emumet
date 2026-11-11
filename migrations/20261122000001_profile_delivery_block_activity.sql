-- D8R: 個別 actor の block は AP `Block` を相手へ配送する (ユーザー決定)。配送 job の種別に 'Block' を加える。
-- P4 が配置した制約 (Follow / Accept / Reject / Undo) を、既存の行を変えずに差し替える。
-- domain block は Block を送らない (use case が job を作らない)。
ALTER TABLE "profile_delivery_jobs" DROP CONSTRAINT "chk_profile_delivery_jobs_activity_type";
ALTER TABLE "profile_delivery_jobs"
  ADD CONSTRAINT "chk_profile_delivery_jobs_activity_type"
    CHECK (activity_type IN ('Follow', 'Accept', 'Reject', 'Undo', 'Block'));
