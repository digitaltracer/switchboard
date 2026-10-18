-- Times are Unix milliseconds.

CREATE TABLE sessions (
    id            TEXT PRIMARY KEY,
    project_id    TEXT NOT NULL,
    machine_id    TEXT NOT NULL,
    agent         TEXT NOT NULL,
    prompt_sha256 TEXT NOT NULL,
    branch        TEXT,
    worktree_path TEXT,
    workspace_id  TEXT,
    pane_id       TEXT,
    lifecycle     TEXT NOT NULL,
    agent_status  TEXT NOT NULL DEFAULT 'unknown',
    revision      INTEGER NOT NULL DEFAULT 0,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL
);
CREATE INDEX sessions_dedupe ON sessions (project_id, machine_id, agent, prompt_sha256, created_at);

CREATE TABLE operations (
    request_id  TEXT PRIMARY KEY,
    kind        TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    session_id  TEXT,
    state       TEXT NOT NULL,
    result      TEXT,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);

CREATE TABLE approvals (
    id          TEXT PRIMARY KEY,
    session_id  TEXT NOT NULL REFERENCES sessions (id),
    tool_name   TEXT NOT NULL,
    tool_input  TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    deadline    INTEGER NOT NULL,
    outcome     TEXT,
    reason      TEXT,
    decided_at  INTEGER
);
CREATE INDEX approvals_pending ON approvals (session_id) WHERE outcome IS NULL;

CREATE TABLE audit (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    ts         INTEGER NOT NULL,
    actor      TEXT NOT NULL,
    session_id TEXT,
    kind       TEXT NOT NULL,
    detail     TEXT NOT NULL
);

CREATE TABLE oauth_clients (
    client_id          TEXT PRIMARY KEY,
    client_secret_hash TEXT,
    redirect_uris      TEXT NOT NULL,
    created_at         INTEGER NOT NULL
);

CREATE TABLE oauth_codes (
    code_hash      TEXT PRIMARY KEY,
    client_id      TEXT NOT NULL REFERENCES oauth_clients (client_id),
    redirect_uri   TEXT NOT NULL,
    code_challenge TEXT NOT NULL,
    resource       TEXT,
    expires_at     INTEGER NOT NULL,
    used_at        INTEGER
);

CREATE TABLE oauth_tokens (
    token_hash TEXT PRIMARY KEY,
    kind       TEXT NOT NULL CHECK (kind IN ('access', 'refresh')),
    client_id  TEXT NOT NULL REFERENCES oauth_clients (client_id),
    family_id  TEXT NOT NULL,
    resource   TEXT,
    expires_at INTEGER NOT NULL,
    revoked_at INTEGER
);
CREATE INDEX oauth_tokens_family ON oauth_tokens (family_id);
