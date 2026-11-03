-- 2. Upgrade character table to allow 'reborn' in CHECK (ruleset IN ('coa', 'wildcard', 'reborn'))
-- Foreign keys are off during migration runner, but legacy child tables remain valid because primary keys are untouched.
CREATE TABLE character_new (
    character_id TEXT PRIMARY KEY,
    profile_id   TEXT NOT NULL REFERENCES profile(profile_id),
    ruleset      TEXT NOT NULL CHECK (ruleset IN ('coa', 'wildcard', 'reborn')),
    name         TEXT NOT NULL,
    race         TEXT NOT NULL,
    class        TEXT NOT NULL,
    gender       INTEGER NOT NULL,
    level        INTEGER NOT NULL,
    revision     INTEGER NOT NULL CHECK (revision >= 1),
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL,
    archived     INTEGER NOT NULL DEFAULT 0 CHECK (archived IN (0, 1))
) STRICT;

INSERT INTO character_new (character_id, profile_id, ruleset, name, race, class, gender, level, revision, created_at, updated_at, archived)
SELECT character_id, profile_id, ruleset, name, race, class, gender, level, revision, created_at, updated_at, archived
FROM character;

DROP TABLE character;
ALTER TABLE character_new RENAME TO character;

CREATE INDEX character_profile ON character(profile_id);
CREATE INDEX character_profile_ruleset ON character(profile_id, ruleset);
