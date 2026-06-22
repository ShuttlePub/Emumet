ALTER TABLE remote_accounts ADD COLUMN IF NOT EXISTS inbox_url TEXT;
ALTER TABLE remote_accounts ADD COLUMN IF NOT EXISTS public_key_pem TEXT;
