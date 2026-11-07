-- The owner's name for a session, from "name it <x>" when it was started; NULL shows `<project>-<id>`.
ALTER TABLE sessions ADD COLUMN name TEXT;
