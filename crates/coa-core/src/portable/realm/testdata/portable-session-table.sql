-- The runtime session table of the core (data/sql/updates/pending_db_characters/rev_20261006_01_coa_portable_session.sql), for the
-- disposable realms of the live tests. Applied after the realm fixture; idempotent.
CREATE TABLE IF NOT EXISTS acore_characters.`coa_portable_session` (
  `guid` INT UNSIGNED NOT NULL,
  `session_id` CHAR(36) NOT NULL,
  `character_id` CHAR(36) NOT NULL,
  `imported_revision` INT UNSIGNED NOT NULL,
  `baseline_generation` INT UNSIGNED NOT NULL DEFAULT 1,
  `state` TINYINT UNSIGNED NOT NULL DEFAULT 0,
  `checkpoint_seq` INT UNSIGNED NOT NULL DEFAULT 0,
  `save_seq` INT UNSIGNED NOT NULL DEFAULT 0,
  `updated_at` INT UNSIGNED NOT NULL DEFAULT 0,
  PRIMARY KEY (`guid`),
  UNIQUE KEY `idx_session_id` (`session_id`)
);
