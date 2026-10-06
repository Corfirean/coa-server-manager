#!/usr/bin/env python3
"""Generate the disposable realm fixture used by the Phase 2 exporter tests.

    python tools/gen_portable_fixture.py > crates/coa-core/src/portable/realm/testdata/realm-fixture.sql

The SQL is applied to an *empty* CoA characters/auth schema of a throw-away MySQL (never to a real realm). It creates
synthetic characters that cover what the exporter has to understand; the recorded answers of the exporter against this
data live next to it in `testdata/golden/`. Text is written as hex literals so no escaping can go wrong.
"""

import binascii
import random

rng = random.Random(20261006)  # deterministic
out = []


def add(sql):
    out.append(sql.rstrip(";") + ";")


def hx(b):
    return "0x" + binascii.hexlify(b).decode()


def T(s):
    """A text literal (utf8mb4) as a hex literal."""
    return "_utf8mb4 " + hx(s.encode("utf8")) if s else "''"


def B(b):
    return hx(b) if b else "''"


def insert(table, cols, rows):
    if not rows:
        return
    values = ",\n  ".join("(" + ", ".join(str(v) for v in row) + ")" for row in rows)
    add(f"INSERT INTO acore_characters.`{table}` ({', '.join('`' + c + '`' for c in cols)}) VALUES\n  {values}")


COLS = ["guid", "account", "name", "race", "class", "gender", "level", "xp", "money", "skin", "face", "hairStyle", "hairColor",
        "facialStyle", "bankSlots", "playerFlags", "taximask", "online", "arenaPoints", "totalHonorPoints", "todayHonorPoints",
        "yesterdayHonorPoints", "totalKills", "todayKills", "yesterdayKills", "chosenTitle", "knownCurrencies", "watchedFaction",
        "talentGroupsCount", "activeTalentGroup", "exploredZones", "knownTitles", "actionBars", "extraBonusTalentCount",
        "resettalents_cost", "stable_slots", "innTriggerId", "deleteDate"]


def character(guid, account, name, race=1, cls=12, gender=0, level=1, **kw):
    d = dict(xp=0, money=0, skin=1, face=2, hair=3, haircolor=4, facial=0, bank=0, flags=0, taxi="0 0 0 0 ", online=0, arena=0,
             honor=0, today_honor=0, yday_honor=0, kills=0, today_kills=0, yday_kills=0, title=0, currencies=0, watched=0,
             groups=1, active=0, explored="0 0 0 0 ", titles="0 0 0 0 0 0 ", bars=0, extra=0, reset=0, stable=0, deleted="NULL")
    d.update(kw)
    insert("characters", COLS, [[guid, account, T(name), race, cls, gender, level, d["xp"], d["money"], d["skin"], d["face"], d["hair"],
                                  d["haircolor"], d["facial"], d["bank"], d["flags"], T(d["taxi"]), d["online"], d["arena"], d["honor"],
                                  d["today_honor"], d["yday_honor"], d["kills"], d["today_kills"], d["yday_kills"], d["title"],
                                  d["currencies"], d["watched"], d["groups"], d["active"], T(d["explored"]), T(d["titles"]), d["bars"],
                                  d["extra"], d["reset"], d["stable"], 0, d["deleted"]]])


# ---- auth accounts -------------------------------------------------------------------------------------------------
for aid, user in [(5001, "PLAYER1"), (5002, "COABOTHOST1"), (5003, "PLAYER3")]:
    add(f"INSERT INTO acore_auth.account (id, username, salt, verifier) VALUES ({aid}, '{user}', UNHEX(REPEAT('11',32)), UNHEX(REPEAT('22',32)))")

add("CREATE TABLE IF NOT EXISTS acore_characters.mod_unknown_state (guid INT UNSIGNED NOT NULL, note VARCHAR(32) NOT NULL, PRIMARY KEY (guid))")

# ---- 1001 naked level 1 ---------------------------------------------------------------------------------------------
character(1001, 5001, "Naked")

# ---- 1002 geared level 80: equipment, bags, enchants, gems, texts, charges, gift, orphans ---------------------------------
explored = " ".join(str((i * 7919) % 100003) for i in range(128)) + " "
character(1002, 5001, "Geared", race=2, cls=21, gender=1, level=80, xp=12345, money=200060523, bank=4, flags=0x0404, arena=150,
          honor=4092, today_honor=10, yday_honor=20, kills=777, today_kills=3, yday_kills=4, title=5, currencies=165, watched=72,
          explored=explored, titles="1 0 0 0 0 0 ", bars=15, extra=2, reset=100000, stable=2)
inst_cols = ["guid", "itemEntry", "owner_guid", "creatorGuid", "giftCreatorGuid", "count", "duration", "charges", "flags", "enchantments",
             "randomPropertyId", "durability", "playedTime", "text"]
inv_cols = ["guid", "bag", "slot", "item"]


def enchants(**slots):
    toks = [[0, 0, 0] for _ in range(12)]
    for k, v in slots.items():
        toks[int(k[1:])] = list(v)
    return " ".join(str(x) for t in toks for x in t) + " "


insts, invs = [], []
g = 20000


def item(entry, bag, slot, count=1, flags=1, ench=None, rand=0, dur=100, creator=0, charges="0 0 0 0 0 ", text="NULL", duration=0, played=3600):
    global g
    g += 1
    insts.append([g, entry, 1002, creator, 0, count, duration, "NULL" if charges is None else T(charges), flags, T(ench or enchants()), rand, dur, played,
                  text if text == "NULL" else T(text)])
    invs.append([1002, bag, slot, g])
    return g


equipment = []
for slot in range(19):
    kw = {}
    if slot % 3 == 0:
        kw = dict(e0=(3000 + slot, 0, 0), e2=(3878, 0, 0), e3=(3879, 0, 0), e4=(3880, 0, 0))
    equipment.append(item(30000 + slot * 17, 0, slot, ench=enchants(**kw) if kw else None, rand=-57 if slot == 5 else 0,
                          dur=90 + slot, creator=1001 if slot == 4 else 0))
bags = [item(41000 + b, 0, b, flags=1) for b in range(19, 23)]
for bi, bag_guid in enumerate(bags):
    for s in range(6):
        item(50000 + bi * 10 + s, bag_guid, s, count=1 + (s % 3) * 4, flags=0)
for s in range(23, 31):
    item(60000 + s, 0, s, count=20, flags=0)
item(70001, 0, 39, text="A book.\nSecond line\twith a tab, a \"quote\", an 'apostrophe', a \\ backslash and 中文.")
item(70002, 0, 40, charges="-1 0 0 0 0 ", flags=0, count=5, duration=3600)
item(70003, 0, 41, charges=None, flags=0)
item(375250, 0, 118, count=12, flags=0)  # currency token slot
wrapped = item(5042, 0, 42, flags=8)
# orphans the realm itself would clean up
invs.append([1002, 0, 50, 20999])            # inventory row without an item_instance
g += 1
insts.append([29998, 70099, 1002, 0, 0, 1, 0, T("0 0 0 0 0 "), 0, T(enchants()), 0, 0, 0, "NULL"])
invs.append([1002, 29999, 3, 29998])         # inside a bag that is not on the character
item(70004, 0, 80, flags=0)                  # buyback slot
insert("item_instance", inst_cols, insts)
insert("character_inventory", inv_cols, invs)
insert("character_gifts", ["guid", "item_guid", "entry", "flags"], [[1002, wrapped, 8000, 8]])
insert("character_spell", ["guid", "spell", "specMask"], [[1002, 500000 + n * 3, 3 if n % 5 == 0 else 1] for n in range(1, 61)])
insert("character_reputation", ["guid", "faction", "standing", "flags"], [[1002, f * 3, f * 100 - 3000, 17 if f % 7 == 0 else 1] for f in range(1, 21)])
add("INSERT INTO acore_characters.character_homebind (guid, mapId, zoneId, posX, posY, posZ) VALUES (1002, 0, 12, 1.5, 2.5, 3.5)")
add("INSERT INTO acore_characters.character_aura (guid, casterGuid, itemGuid, spell, effectMask, recalculateMask, stackCount, maxDuration, remainTime, remainCharges) VALUES (1002, 1002, 0, 12345, 1, 0, 1, 60000, 30000, 0)")
add("INSERT INTO acore_characters.character_spell_cooldown (guid, spell, category, item, time, needSend) VALUES (1002, 500003, 0, 0, 1790000000, 0)")

# ---- 1003 quests, reputation, action bars ---------------------------------------------------------------------------------
character(1003, 5001, "Questy", race=3, cls=13, level=40, xp=900, money=12345)
insert("character_queststatus", ["guid", "quest", "status", "explored", "timer", "mobcount1", "mobcount2", "mobcount3", "mobcount4",
                                 "itemcount1", "itemcount2", "itemcount3", "itemcount4", "itemcount5", "itemcount6", "playercount"],
       [[1003, 12001, 3, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], [1003, 12002, 1, 1, 600, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0]])
insert("character_queststatus_rewarded", ["guid", "quest", "active"], [[1003, 12000, 1], [1003, 12003, 1], [1003, 12004, 1], [1003, 12005, 0]])
insert("character_queststatus_daily", ["guid", "quest", "time"], [[1003, 13000, 1790000000]])
insert("character_reputation", ["guid", "faction", "standing", "flags"], [[1003, f, f * 50, 1] for f in range(1, 11)])
insert("character_action", ["guid", "spec", "button", "action", "type"],
       [[1003, 0, b, 500003 + b * 3, 0] for b in range(12)] + [[1003, 1, b, 500100 + b, 0] for b in range(4)] + [[1003, 1, 4, 7, 64]])

# ---- 1004 build: spells, talents, glyphs, skills, settings, macros -------------------------------------------------------------
character(1004, 5001, "Дракон", race=8, cls=25, level=80, money=5000000, titles="0 0 0 0 0 0 ")
insert("character_spell", ["guid", "spell", "specMask"], [[1004, 500000 + n * 3, [1, 2, 3][n % 3]] for n in range(1, 301)])
insert("character_talent", ["guid", "spell", "specMask"], [[1004, 900000 + n, 1] for n in range(1, 6)])
insert("character_glyphs", ["guid", "talentGroup", "glyph1", "glyph2", "glyph3", "glyph4", "glyph5", "glyph6"], [[1004, 0, 1, 2, 3, 4, 5, 6], [1004, 1, 7, 0, 0, 0, 0, 0]])
insert("character_skills", ["guid", "skill", "value", "max"], [[1004, 100 + n, 300 + n, 450] for n in range(10)])
insert("character_settings", ["guid", "source", "data"], [
    [1004, T("core.ascension_active_spec"), T("54 ")],
    [1004, T("core.ascension_starter"), T("1 ")],
    [1004, T("core.ascension_build.54"), T("3 56710 56320 90101 ")],
    [1004, T("core.ascension_bar.54"), T("2 0 500003 1 500006 ")],
    [1004, T("core.ascension_slot.active"), T("0 ")],
    [1004, T("core.spell_charge.804197"), T("2 1790000000 ")],
    [1004, T("coa.bot.gear"), T("1 2 3 ")],
    [1004, T("coa.gear_pref_weapon"), T("2 ")],
    [1004, T("mod.example.new_feature"), T("9 8 7 ")],
    [1004, T("core.wildcard.cards"), T("1 2 ")],
])
macro_blob = bytes(rng.randrange(256) for _ in range(600)) + b"\x00\x01\xff end"
insert("character_account_data", ["guid", "type", "time", "data"], [[1004, 5, 1790000000, B(macro_blob)], [1004, 1, 1790000001, B(b"ignored config cache")]])

# ---- 1005 pets ---------------------------------------------------------------------------------------------------------------
character(1005, 5001, "Hunty", race=4, cls=14, level=60, stable=2)
pet_cols = ["id", "entry", "owner", "modelid", "CreatedBySpell", "PetType", "level", "exp", "Reactstate", "name", "renamed", "slot",
            "curhealth", "curmana", "curhappiness", "savetime", "abdata"]
insert("character_pet", pet_cols, [
    [3001, 42, 1005, 901, 1515, 1, 60, 1000, 1, T("Rex"), 0, 0, 5000, 100, 1000000, 1790000000, T("7 2 0 0 1 0 1 1 0 ")],
    [3002, 43, 1005, 902, 1515, 1, 55, 200, 1, T("Fang"), 1, 1, 4000, 90, 900000, 1790000000, T("7 2 ")],
    [3003, 416, 1005, 4449, 688, 0, 60, 0, 1, T("Imp"), 0, 100, 3000, 400, 0, 1790000000, "NULL"],
])
insert("pet_spell", ["guid", "spell", "active"], [[3001, s, a] for s, a in [(17253, 1), (16827, 0), (24604, 1), (61683, 0), (34889, 1), (50256, 0)]] + [[3003, 3110, 1]])
insert("character_pet_declinedname", ["id", "owner", "genitive", "dative", "accusative", "instrumental", "prepositional"],
       [[3002, 1005, T("Фанга"), T("Фангу"), T("Фанг"), T("Фангом"), T("Фанге")]])
add("INSERT INTO acore_characters.pet_aura (guid, casterGuid, spell, effectMask, recalculateMask, stackCount, maxDuration, remainTime, remainCharges) VALUES (3001, 1005, 99, 1, 0, 1, 1000, 500, 0)")

# ---- blocked characters -------------------------------------------------------------------------------------------------------
character(1006, 5001, "Online", level=10, online=1)
character(1007, 5001, "Hardcore", level=10)
add("INSERT INTO acore_characters.coa_character_gamemode (guid, gameMode) VALUES (1007, 1)")
add("INSERT INTO acore_characters.coa_character_challenge (guid, challengeId, level, deaths, hunger, thirst, startTime) VALUES (1007, 5, 1, 0, 90, 90, 1790000000)")
character(1008, 5001, "Cachey", level=30)
add("INSERT INTO acore_characters.item_instance (guid, itemEntry, owner_guid, enchantments) VALUES (30001, 900001, 1008, '0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 ')")
add("INSERT INTO acore_characters.ascension_manastorm_cache (item, guid) VALUES (30001, 1008)")
character(1009, 5001, "Deleted", level=20, deleted=1790000000)
character(1010, 5002, "Botty", level=80)
character(1011, 5003, "Moddy", level=50)
add("INSERT INTO acore_characters.mod_unknown_state (guid, note) VALUES (1011, 'data this Manager does not know')")

# ---- characters that look like the blocked ones but are NOT blocked ---------------------------------------------------------------
character(1012, 5001, "History", level=60)
add("INSERT INTO acore_characters.coa_character_gamemode (guid, gameMode) VALUES (1012, 0)")
add("INSERT INTO acore_characters.coa_challenge_completion (guid, challengeId, level, completeTime, startTime) VALUES (1012, 5, 1, 1790000100, 1790000000)")
add("INSERT INTO acore_characters.coa_character_condition (guid, flag) VALUES (1012, 'finished_once')")

print("-- generated by tools/gen_portable_fixture.py; applied to a disposable, empty characters + auth schema only")
print("\n".join(out))
