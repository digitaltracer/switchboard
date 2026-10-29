-- Phone answers: the `waiting` notification, the attentions it points to, and answer challenges.
-- SQLite can't alter a CHECK, so `outbox` and `challenges` are rebuilt with every row kept.
-- Renaming a parent table repoints its children, so `outbox_deliveries` is rebuilt too.

ALTER TABLE outbox RENAME TO outbox_old;
CREATE TABLE outbox (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    -- One row per event, e.g. `approval:<id>`, `finished:<session>:<revision>` or `waiting:<attention>`.
    dedupe_key   TEXT NOT NULL UNIQUE,
    kind         TEXT NOT NULL CHECK (kind IN ('approval', 'finished', 'device_paired', 'waiting')),
    session_id   TEXT,
    approval_id  TEXT,
    created_at   INTEGER NOT NULL,
    attention_id TEXT
);
INSERT INTO outbox (id, dedupe_key, kind, session_id, approval_id, created_at)
    SELECT id, dedupe_key, kind, session_id, approval_id, created_at FROM outbox_old;

ALTER TABLE outbox_deliveries RENAME TO outbox_deliveries_old;
CREATE TABLE outbox_deliveries (
    outbox_id       INTEGER NOT NULL REFERENCES outbox (id),
    device_id       TEXT NOT NULL REFERENCES devices (id),
    state           TEXT NOT NULL CHECK (state IN ('pending', 'sent', 'failed', 'dropped')),
    attempts        INTEGER NOT NULL DEFAULT 0,
    next_attempt_at INTEGER NOT NULL,
    last_error      TEXT,
    sent_at         INTEGER,
    PRIMARY KEY (outbox_id, device_id)
);
INSERT INTO outbox_deliveries SELECT outbox_id, device_id, state, attempts, next_attempt_at,
    last_error, sent_at FROM outbox_deliveries_old;
DROP TABLE outbox_deliveries_old;
DROP TABLE outbox_old;
CREATE INDEX outbox_deliveries_due ON outbox_deliveries (next_attempt_at) WHERE state = 'pending';

CREATE TABLE challenges_new (
    nonce       TEXT PRIMARY KEY,
    device_id   TEXT NOT NULL REFERENCES devices (id),
    purpose     TEXT NOT NULL CHECK (purpose IN ('token', 'approval', 'answer')),
    -- The approval, or for an answer the dialog occurrence, the nonce is for.
    approval_id TEXT,
    created_at  INTEGER NOT NULL,
    expires_at  INTEGER NOT NULL,
    used_at     INTEGER
);
INSERT INTO challenges_new SELECT nonce, device_id, purpose, approval_id, created_at, expires_at, used_at
    FROM challenges;
DROP TABLE challenges;
ALTER TABLE challenges_new RENAME TO challenges;

-- An agent waiting at a dialog, as a `waiting` push names it. It resolves only to the pane
-- incarnation and occurrence it was made for, never to a replacement agent.
CREATE TABLE attention (
    id            TEXT PRIMARY KEY,
    machine_id    TEXT NOT NULL,
    herdr_session TEXT,
    agent         TEXT NOT NULL,
    terminal_id   TEXT NOT NULL,
    session_id    TEXT,
    occurrence    TEXT NOT NULL,
    created_at    INTEGER NOT NULL,
    expires_at    INTEGER NOT NULL,
    resolved_at   INTEGER,
    -- `answered`, `gone` or `expired`.
    resolution    TEXT,
    UNIQUE (machine_id, agent, occurrence)
);
CREATE INDEX attention_open ON attention (machine_id, agent) WHERE resolved_at IS NULL;
