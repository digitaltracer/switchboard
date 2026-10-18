-- Hub state: notification mutes shared with the WhatsApp bridge, hook failures, approval delivery.

ALTER TABLE sessions ADD COLUMN agent_name TEXT;
ALTER TABLE sessions ADD COLUMN muted INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN hook_failed INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN error TEXT;
CREATE UNIQUE INDEX sessions_agent_name ON sessions (agent_name);

-- Whether the runner took the decision: delivered, not_delivered (the tool did not run), unknown.
ALTER TABLE approvals ADD COLUMN delivery TEXT;

CREATE TABLE notify_settings (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    mute_all    INTEGER NOT NULL DEFAULT 0,
    -- When the bridge's own watch file was imported; it is imported once.
    imported_at INTEGER
);
INSERT INTO notify_settings (id) VALUES (1);

-- Agents named by the bridge that are not Switchboard sessions: opted in, muted, or both.
CREATE TABLE notify_agents (
    name       TEXT PRIMARY KEY,
    watched    INTEGER NOT NULL DEFAULT 0,
    muted      INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL
);
