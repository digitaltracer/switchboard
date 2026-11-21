-- What agents did, as the hub saw it: a turn started, finished, waited at a dialog, or stopped on
-- an error. "While you were away" reads it back; rows older than a week are dropped.
CREATE TABLE agent_events (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    machine_id TEXT NOT NULL,
    agent      TEXT NOT NULL,
    session_id TEXT,
    kind       TEXT NOT NULL CHECK (kind IN ('working', 'finished', 'waiting', 'error')),
    at         INTEGER NOT NULL
);
CREATE INDEX agent_events_at ON agent_events (at);

-- Hub-wide settings the phones change, as JSON values by key.
CREATE TABLE hub_settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
