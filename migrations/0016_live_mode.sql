-- When a phone's lock-screen card shows: `working` (while something works or waits, ending
-- fifteen quiet minutes later) or `always`.
ALTER TABLE live_registrations ADD COLUMN mode TEXT NOT NULL DEFAULT 'working' CHECK (mode IN ('working', 'always'));
