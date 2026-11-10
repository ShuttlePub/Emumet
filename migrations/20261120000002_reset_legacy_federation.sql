-- 旧 Account 単位の連合データだけを初期化する破壊的 migration (PF / P4 `profile-federation-in-place-cutover`、
-- ADR profile-federation-in-place-reset)。P2 が検証した初期化 SQL を本番 migration にしたもの。
--
-- !! 不可逆 !! 新バイナリの起動 (= migration の自動適用) が実行トリガになる。適用の前に必ず、
-- 旧プロセス (旧 inbox・Account 単位の関係書込・旧配送 worker) を停止し、バックアップを取り、
-- docs/profile-federation-cutover.md の手順どおりに行うこと。
--
-- 対象 (旧連合データ。Account 単位の連合 identity・関係・配送状態・連合鍵):
--   follows            旧 Account 単位の Follow
--   blocks             旧 Account 単位の block
--   mutes              旧 Account 単位の mute
--   remote_accounts    旧連合 identity (リモート actor の取り込み)
--   outbox_activities  旧 Account 単位の配送状態
--   signing_keys       旧 Account に紐づく連合鍵 (新 Profile 鍵は profile_signing_keys に一回限り生成する)
--
-- 保持するもの (一切触れない): auth_hosts, auth_accounts, auth_emumet_accounts, accounts, profiles,
-- metadatas, images, organization_members, profile_transfer_requests (+ events), account_events,
-- profile_events, metadata_events, account_report_events, account_reports, registration_gate_*,
-- projection_checkpoints, profile_domain_controls, profile_signing_keys、および新モデル (profile_follows 等)。
--
-- 安全性:
--   * TRUNCATE ... CASCADE は使わない。対象テーブルを明示列挙する。対象を参照する外部キーが対象外の
--     テーブルにあれば TRUNCATE は失敗する (黙って巻き込まない)。
--   * 前提検査: username が未設定の Profile が 1 件でも残っていれば RAISE EXCEPTION で abort する。
--     migration は 1 transaction なので、abort 時は何も消えず台帳も残らない (切替拒否)。
--   * 実行済みかどうかを profile_federation_resets に記録し、再実行しても再消去しない。
--     記録の確認・前提検査・消去・記録は同じ transaction で行う。

CREATE TABLE IF NOT EXISTS "profile_federation_resets" (
  "reset_id" TEXT PRIMARY KEY,
  "applied_at" TIMESTAMPTZ NOT NULL DEFAULT now()
);

DO $reset$
DECLARE
  missing_username BIGINT;
BEGIN
  IF NOT EXISTS (
    SELECT 1 FROM profile_federation_resets WHERE reset_id = 'legacy-account-federation-v1'
  ) THEN
    SELECT count(*) INTO missing_username FROM profiles WHERE username IS NULL;
    IF missing_username > 0 THEN
      RAISE EXCEPTION
        'profile federation cutover refused: % profile(s) have no username. Set a username for every existing Profile (POST /api/v1/profiles/{profile_nanoid}/username) and restart. Nothing was changed.',
        missing_username
        USING ERRCODE = 'check_violation';
    END IF;

    TRUNCATE TABLE follows, blocks, mutes, remote_accounts, outbox_activities, signing_keys;
    INSERT INTO profile_federation_resets (reset_id) VALUES ('legacy-account-federation-v1');
  END IF;
END
$reset$;
