-- A phone's lock-screen card. iOS gives a live key (boards are sealed to it) and its activity's
-- push tokens; Android needs neither, since it fetches the board when woken.
CREATE TABLE live_registrations (
    device_id           TEXT PRIMARY KEY REFERENCES devices (id),
    -- P-256 key-agreement key, SEC1 uncompressed, base64url.
    live_key            TEXT,
    -- iOS 17.2's push-to-start token, for starting a card while the app isn't running.
    start_token         TEXT,
    -- The running card's update token.
    activity_token      TEXT,
    -- Unix seconds the running card started.
    activity_started_at INTEGER,
    updated_at          INTEGER NOT NULL
);
