-- Add UNIQUE constraint on accounts.name for ActivityPub WebFinger identity
DO $$ BEGIN
    ALTER TABLE accounts ADD CONSTRAINT accounts_name_key UNIQUE (name);
EXCEPTION
    WHEN duplicate_object THEN null;
END $$;
