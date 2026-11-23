-- The id of the space a session's agent was last seen in (a Herdr app space, or a Herdr workspace), so the phone can rename the space or start another session in it.
ALTER TABLE sessions ADD COLUMN space_id TEXT;
