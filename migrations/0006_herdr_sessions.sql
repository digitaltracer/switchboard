-- The Herdr session each task runs in; each new task gets one of its own.
ALTER TABLE sessions ADD COLUMN herdr_session TEXT;
