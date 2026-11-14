-- remote actor の表示情報 (preferredUsername / name / icon) の永続キャッシュ (D08 ui-spec 1-6、
-- `profile-relation-actor-display`)。個別 block・mute 一覧の `target_actor` がここから返す。
--
-- 非破壊の追加 (新テーブルのみ)。旧連合データの初期化 (20261120000002) とは独立で、既存テーブルには触れない。
--
-- 設計:
--   * 公開されている actor 情報だけを持つ。所有者・関係の情報は持たず、Profile への外部キーもない
--     (別の所有者が同じ actor を対象にしても取得は 1 回に共有される。返る関係は常に所有者自身の設定だけ)。
--   * 取得は一覧の応答の外 (block/mute 作成後と一覧応答後のバックグラウンド) で行う。`next_refresh_at` は
--     「次に取得してよい時刻」で、取得開始時の短いリース (60 秒) と、結果の記録時の再取得時刻 (成功 24 時間・
--     失敗 15 分) を兼ねる。リースの確保は `INSERT ... ON CONFLICT DO UPDATE ... WHERE next_refresh_at <= now`
--     の原子的な 1 文で、複数リクエスト・複数プロセスが同じ actor を同時に要求しても取得は 1 回になる。
--   * status: pending = 未取得 (リース確保済み)、resolved = 取得済み、failed = 直近の取得が失敗
--     (resolved だった行は、更新の失敗でも古い情報と status を保つ)。
CREATE TABLE "profile_remote_actor_cache" (
  "actor_url" TEXT PRIMARY KEY,
  "status" TEXT NOT NULL,
  "preferred_username" TEXT,
  "display_name" TEXT,
  "icon_url" TEXT,
  "fetched_at" TIMESTAMPTZ,
  "next_refresh_at" TIMESTAMPTZ NOT NULL,
  CONSTRAINT chk_profile_remote_actor_cache_url_length
    CHECK (octet_length(actor_url) <= 1024),
  CONSTRAINT chk_profile_remote_actor_cache_icon_length
    CHECK (octet_length(icon_url) <= 1024),
  CONSTRAINT chk_profile_remote_actor_cache_status
    CHECK (status IN ('pending', 'resolved', 'failed')),
  CONSTRAINT chk_profile_remote_actor_cache_resolved
    CHECK (status <> 'resolved' OR (preferred_username IS NOT NULL AND fetched_at IS NOT NULL))
);
