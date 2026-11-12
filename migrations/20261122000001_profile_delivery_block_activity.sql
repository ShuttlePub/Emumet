-- D8R: 個別 actor の block は AP `Block` を相手へ配送する (ユーザー決定)。配送 job の種別に 'Block' を加える。
-- P4 が配置した制約 (Follow / Accept / Reject / Undo) を、既存の行を変えずに差し替える。
-- domain block は Block を送らない (use case が job を作らない)。
ALTER TABLE "profile_delivery_jobs" DROP CONSTRAINT "chk_profile_delivery_jobs_activity_type";
ALTER TABLE "profile_delivery_jobs"
  ADD CONSTRAINT "chk_profile_delivery_jobs_activity_type"
    CHECK (activity_type IN ('Follow', 'Accept', 'Reject', 'Undo', 'Block'));

-- 個別 block の actor 対 (送信元 Profile x 相手 actor) ごとの「後始末・再 block をこれより前に送らない」下限。
-- block / unblock を繰り返すとき、取消した Block / Undo(Block) が送信中だった可能性があれば、それが相手に着地し
-- 得る最後の時刻 (取消完了後に取り直した時刻 + 猶予、取消時点の lease の期限の遅い方) まで、次に予約する
-- Block / Undo(Block) を始めない。値は単調非減少 (GREATEST で更新) で、操作の連鎖で短縮されない。
CREATE TABLE "profile_block_settle" (
  "sender_profile_id" BIGINT NOT NULL REFERENCES "profiles" ("id") ON DELETE CASCADE,
  "target_actor_url" TEXT NOT NULL,
  "settle_not_before" TIMESTAMPTZ NOT NULL,
  PRIMARY KEY ("sender_profile_id", "target_actor_url"),
  CONSTRAINT chk_profile_block_settle_url_length CHECK (octet_length(target_actor_url) <= 1024)
);
