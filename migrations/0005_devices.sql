-- Paired app devices: pairing codes, device keys, challenges, short-lived tokens, push deliveries.

-- Names this hub in every signed message, so a signature for one hub is useless at another.
CREATE TABLE hub_identity (
    id     INTEGER PRIMARY KEY CHECK (id = 1),
    hub_id TEXT NOT NULL
);
INSERT INTO hub_identity (id, hub_id) VALUES (1, lower(hex(randomblob(16))));

-- Single-use codes from `switchboard admin pair-device`; only the hash is kept.
CREATE TABLE pairing_codes (
    code_hash  TEXT PRIMARY KEY,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    claimed_at INTEGER,
    device_id  TEXT
);

-- Keys are P-256 public keys, SEC1 uncompressed, base64url. `pending` waits for the fingerprint check.
CREATE TABLE devices (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL,
    auth_key      TEXT NOT NULL,
    approval_key  TEXT NOT NULL,
    fingerprint   TEXT NOT NULL,
    state         TEXT NOT NULL CHECK (state IN ('pending', 'active', 'rejected', 'revoked')),
    client_build  INTEGER,
    created_at    INTEGER NOT NULL,
    confirmed_at  INTEGER,
    revoked_at    INTEGER,
    last_seen_at  INTEGER,
    push_token    TEXT,
    push_env      TEXT CHECK (push_env IN ('sandbox', 'production')),
    push_token_at INTEGER
);

-- Nonces a device signs: for a token, or for one approval. Used once, before `expires_at`.
CREATE TABLE challenges (
    nonce       TEXT PRIMARY KEY,
    device_id   TEXT NOT NULL REFERENCES devices (id),
    purpose     TEXT NOT NULL CHECK (purpose IN ('token', 'approval')),
    approval_id TEXT,
    created_at  INTEGER NOT NULL,
    expires_at  INTEGER NOT NULL,
    used_at     INTEGER
);

-- Short-lived bearer tokens for the app API, stored as SHA-256 hex.
CREATE TABLE device_tokens (
    token_hash TEXT PRIMARY KEY,
    device_id  TEXT NOT NULL REFERENCES devices (id),
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);
CREATE INDEX device_tokens_device ON device_tokens (device_id);

-- One row per notification and device, created with the notification, so no device gets one twice.
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
CREATE INDEX outbox_deliveries_due ON outbox_deliveries (next_attempt_at) WHERE state = 'pending';
