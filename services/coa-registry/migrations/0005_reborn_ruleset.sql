-- Registry schema, migration 5: add 'reborn' to supported rulesets.
ALTER TABLE realms DROP CONSTRAINT IF EXISTS realms_ruleset_check;
ALTER TABLE realms ADD CONSTRAINT realms_ruleset_check CHECK (ruleset IN ('coa', 'wildcard', 'reborn'));
