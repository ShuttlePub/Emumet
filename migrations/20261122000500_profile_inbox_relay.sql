-- An UndoFollow marker is cancellation state, never proof that a Follow was received.
-- Keep its activity_type unchanged for the existing early-Undo protection, but separately
-- remember an actual authenticated Follow/Block even if it arrived after the marker.
ALTER TABLE profile_inbound_activities ADD COLUMN observed_relation boolean NOT NULL DEFAULT false;
UPDATE profile_inbound_activities SET observed_relation = true WHERE activity_type IN ('Follow', 'Block');
