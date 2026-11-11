-- D8R: 個別 actor の block は AP `Block` を相手へ配送する (ユーザー決定)。配送 job の種別に 'Block' を加える。
-- P4 が配置した制約 (Follow / Accept / Reject / Undo) を、既存の行を変えずに差し替える。
-- domain block は Block を送らない (use case が job を作らない)。
ALTER TABLE "profile_delivery_jobs" DROP CONSTRAINT "chk_profile_delivery_jobs_activity_type";
ALTER TABLE "profile_delivery_jobs"
  ADD CONSTRAINT "chk_profile_delivery_jobs_activity_type"
    CHECK (activity_type IN ('Follow', 'Accept', 'Reject', 'Undo', 'Block'));

-- 取消した瞬間の lease の期限。取消は lease を消す (state <> 'in_flight' では lease 列が NULL の制約) が、
-- 送信中だった job は取消後も lease の期限まで相手に着地し得る。block / unblock を繰り返すとき、後始末の
-- 開始時刻の下限を取消の連鎖の中でも失わないよう、取消した job に期限を残す。
ALTER TABLE "profile_delivery_jobs" ADD COLUMN "cancelled_lease_expires_at" TIMESTAMPTZ;
