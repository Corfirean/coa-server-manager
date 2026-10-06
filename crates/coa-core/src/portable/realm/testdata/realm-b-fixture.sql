-- Realm B: the disposable DESTINATION realm of the Phase 3 live tests. Applied to a copy of the schema fixture (which already
-- holds the CoA world data). Never applied to a real realm.
INSERT INTO acore_auth.account (id, username, salt, verifier) VALUES
  (7001, 'PLAYERB', UNHEX(REPEAT('11',32)), UNHEX(REPEAT('22',32))),
  (7002, 'COABOTHOST2', UNHEX(REPEAT('11',32)), UNHEX(REPEAT('22',32))),
  (7003, 'FULLACCT', UNHEX(REPEAT('11',32)), UNHEX(REPEAT('22',32)));
INSERT INTO acore_characters.characters (`guid`, `account`, `name`, `race`, `class`, `gender`, `level`, `taximask`, `innTriggerId`) VALUES
  (2001, 7001, _utf8mb4 0x4e414b4544, 1, 12, 0, 5, '', 0),
  (2002, 7001, _utf8mb4 0x4578697374696e67, 1, 12, 0, 5, '', 0),
  (3001, 7003, _utf8mb4 0x46756c6c30, 1, 12, 0, 1, '', 0),
  (3002, 7003, _utf8mb4 0x46756c6c31, 1, 12, 0, 1, '', 0),
  (3003, 7003, _utf8mb4 0x46756c6c32, 1, 12, 0, 1, '', 0),
  (3004, 7003, _utf8mb4 0x46756c6c33, 1, 12, 0, 1, '', 0),
  (3005, 7003, _utf8mb4 0x46756c6c34, 1, 12, 0, 1, '', 0),
  (3006, 7003, _utf8mb4 0x46756c6c35, 1, 12, 0, 1, '', 0),
  (3007, 7003, _utf8mb4 0x46756c6c36, 1, 12, 0, 1, '', 0),
  (3008, 7003, _utf8mb4 0x46756c6c37, 1, 12, 0, 1, '', 0),
  (3009, 7003, _utf8mb4 0x46756c6c38, 1, 12, 0, 1, '', 0),
  (3010, 7003, _utf8mb4 0x46756c6c39, 1, 12, 0, 1, '', 0);
INSERT INTO acore_characters.reserved_name (name) VALUES (_utf8mb4 0x486973746f7279);
INSERT IGNORE INTO acore_world.creature_template (entry) VALUES (42), (43), (416);
-- the entries the fixture characters use that a real item_template may lack
INSERT IGNORE INTO acore_world.item_template (entry) VALUES (5042), (8000), (30000), (30017), (30034), (30051), (30068), (30085), (30102), (30119), (30136), (30153), (30170), (30187), (30204), (30221), (30238), (30255), (30272), (30289), (30306), (41019), (41020), (41021), (41022), (50000), (50001), (50002), (50003), (50004), (50005), (50010), (50011), (50012), (50013), (50014), (50015), (50020), (50021), (50022), (50023), (50024), (50025), (50030), (50031), (50032), (50033), (50034), (50035), (60023), (60024), (60025), (60026), (60027), (60028), (60029), (60030), (70001), (70002), (70003), (70004), (375250);
