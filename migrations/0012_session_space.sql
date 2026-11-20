-- The space (Herdr app space, or Herdr workspace) a session's agent was last seen in.
ALTER TABLE sessions ADD COLUMN space TEXT;
