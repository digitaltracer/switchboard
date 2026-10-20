-- Groundwork for the app API: a session's starting commit, a durable change counter, the outbox.

-- The commit the worktree was created from; sessions created before this migration have none.
ALTER TABLE sessions ADD COLUMN start_commit TEXT;

-- Bumped by every change a session list shows, so a long-poll survives a hub restart.
CREATE TABLE revisions (
    id    INTEGER PRIMARY KEY CHECK (id = 1),
    value INTEGER NOT NULL
);
INSERT INTO revisions (id, value) SELECT 1, COALESCE(MAX(revision), 0) FROM sessions;

-- Notifications to send, written in the same transaction as the change that causes them.
CREATE TABLE outbox (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    -- One row per event, e.g. `approval:<id>` or `finished:<session>:<revision>`.
    dedupe_key  TEXT NOT NULL UNIQUE,
    kind        TEXT NOT NULL CHECK (kind IN ('approval', 'finished', 'device_paired')),
    session_id  TEXT,
    approval_id TEXT,
    created_at  INTEGER NOT NULL
);
