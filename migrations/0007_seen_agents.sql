-- Herdr agents Switchboard didn't start, kept after they end so the app can show what ended.
CREATE TABLE seen_agents (
    id INTEGER PRIMARY KEY,
    machine_id TEXT NOT NULL,
    herdr_session TEXT,
    pane_id TEXT NOT NULL,
    kind TEXT,
    name TEXT NOT NULL,
    title TEXT,
    first_seen INTEGER NOT NULL,
    last_seen INTEGER NOT NULL,
    ended_at INTEGER
);
CREATE INDEX seen_agents_by_machine ON seen_agents (machine_id, ended_at);
