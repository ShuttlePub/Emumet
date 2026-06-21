-- Add UNIQUE constraint on accounts.name for ActivityPub WebFinger identity
ALTER TABLE accounts ADD CONSTRAINT accounts_name_key UNIQUE (name);
