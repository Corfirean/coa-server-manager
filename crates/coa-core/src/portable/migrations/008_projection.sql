-- Schema 8: level-cap projection (Phase 8).
--
--  realm_projection                          what a character is shown on a realm whose level cap is below the character's level: the
--                                            context of the projection (levels, the canonical revision the working copy was last brought
--                                            to, the content profile and progression signature, the policy version) and the core's
--                                            decision (what is held). Absent: the character is not projected on that realm.
--  character_server_mapping.progression_pin the pin (cap, policy, progression signature, content profile hash, projected or native) the
--                                            character was last synchronised under, projected or not: a realm with another signature has
--                                            another cap or other rules
--  owner_session.progression                 the pin (and, when projected, the context) the Host reported when the session started;
--                                            every later checkpoint must carry the same pin
--  host_session.pin / host_session.reproject the pin this session runs under; reproject = the realm's progression profile changed under
--                                            the session: its final checkpoint was taken under the old pin and the working copy must be
--                                            projected again before the next session is armed

ALTER TABLE character_server_mapping ADD COLUMN progression_pin TEXT;

CREATE TABLE realm_projection (
    character_id         TEXT NOT NULL REFERENCES character(character_id) ON DELETE CASCADE,
    server_id            TEXT NOT NULL,
    canonical_level      INTEGER NOT NULL,
    projected_level      INTEGER NOT NULL,
    canonical_revision   INTEGER NOT NULL,
    content_profile_hash TEXT NOT NULL,
    progression_signature TEXT NOT NULL,
    policy_version       INTEGER NOT NULL,
    context              TEXT NOT NULL,
    updated_at           TEXT NOT NULL,
    PRIMARY KEY (character_id, server_id)
) STRICT;

ALTER TABLE owner_session ADD COLUMN progression TEXT;
ALTER TABLE host_session ADD COLUMN pin TEXT;
ALTER TABLE host_session ADD COLUMN reproject INTEGER NOT NULL DEFAULT 0;
