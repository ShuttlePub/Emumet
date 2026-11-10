-- 旧 Account 単位の連合データだけを初期化する in-place reset の検証用 fixture (PF / P2 → P4 へ引き渡し)。
--
-- これは **隔離 DB 用の fixture** で、本番の migrations/ には配置しない。P4 が versioned migration として
-- 採番・配置する。ここでは対象一覧と安全性 (非対象データを保持する・一回だけ実行する) を検証する。
--
-- 対象 (旧連合データ。Account 単位の連合 identity・関係・配送状態・連合鍵):
--   follows            旧 Account 単位の Follow
--   blocks             旧 Account 単位の block
--   mutes              旧 Account 単位の mute
--   remote_accounts    旧連合 identity (リモート actor の取り込み)
--   outbox_activities  旧 Account 単位の配送状態
--   signing_keys       旧 Account に紐づく連合鍵 (新 Profile 鍵は P1 の保存先に一回限り生成する)
--
-- 保持するもの (一切触れない): auth_hosts, auth_accounts, auth_emumet_accounts, accounts, profiles,
-- metadatas, images, organization_members, profile_transfer_requests (+ events), account_events,
-- profile_events, metadata_events, account_report_events, account_reports, registration_gate_*,
-- projection_checkpoints、および新モデル (profile_follows 等)。
--
-- 安全性:
--   * TRUNCATE ... CASCADE は使わない。対象テーブルを明示列挙する。対象を参照する外部キーが対象外の
--     テーブルにあれば TRUNCATE は失敗する (黙って巻き込まない)。
--   * 実行済みかどうかを profile_federation_resets に記録し、再実行しても再消去しない。
--     記録の確認と消去は同じ transaction で行う。

CREATE TABLE IF NOT EXISTS "profile_federation_resets" (
  "reset_id" TEXT PRIMARY KEY,
  "applied_at" TIMESTAMPTZ NOT NULL DEFAULT now()
);

DO $reset$
BEGIN
  IF NOT EXISTS (
    SELECT 1 FROM profile_federation_resets WHERE reset_id = 'legacy-account-federation-v1'
  ) THEN
    TRUNCATE TABLE follows, blocks, mutes, remote_accounts, outbox_activities, signing_keys;
    INSERT INTO profile_federation_resets (reset_id) VALUES ('legacy-account-federation-v1');
  END IF;
END
$reset$;
