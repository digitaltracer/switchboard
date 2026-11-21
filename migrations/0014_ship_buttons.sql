-- The ship buttons phones show on a finished agent: each asks the agent to run one of its own
-- skills. `scope` is `*` for every project, or a project id whose list replaces it there.
CREATE TABLE ship_buttons (
    scope    TEXT NOT NULL,
    position INTEGER NOT NULL,
    -- The skill's name without its slash, e.g. `pr` or `plugin:review`.
    skill    TEXT NOT NULL,
    label    TEXT NOT NULL,
    -- Shown only once the agent's reply names a pull request.
    after_pr INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (scope, position)
);
