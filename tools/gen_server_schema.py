"""Regenerates schemas/server.json and schemas/presets/server.json (curated worldserver.conf whitelist).
Every key was verified to exist in the repack's Settings/worldserver.conf.template; defaults are the shipped values."""
import json
import os

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
cats = [("gameplay", "Gameplay"), ("rates", "Rates"), ("characters", "Characters"), ("world", "World"), ("network", "Network"),
        ("accounts", "Accounts"), ("logging", "Logging"), ("performance", "Performance"), ("advanced", "Advanced")]
# key, type, category, title, description, default, min, max, unit, advanced, restart, dangerous
R = [
    ("Rate.XP.Kill", "float", "rates", "Experience from kills", "Multiplier for experience gained by killing monsters. 1 is normal.", 1, 0, 20, "x", False, "world", False),
    ("Rate.XP.Quest", "float", "rates", "Experience from quests", "Multiplier for experience gained from quests.", 1, 0, 20, "x", False, "world", False),
    ("Rate.XP.Quest.DF", "float", "rates", "Experience from daily / dungeon-finder quests", "Multiplier for experience from Dungeon Finder quests.", 1, 0, 20, "x", True, "world", False),
    ("Rate.XP.Explore", "float", "rates", "Experience from exploring", "Multiplier for experience from discovering new areas.", 1, 0, 20, "x", False, "world", False),
    ("Rate.Drop.Item.Normal", "float", "rates", "Item drop rate", "Multiplier for how often normal-quality items drop.", 1, 0, 20, "x", False, "world", False),
    ("Rate.Drop.Money", "float", "rates", "Gold drop rate", "Multiplier for gold dropped by monsters.", 1, 0, 20, "x", False, "world", False),
    ("Rate.Honor", "float", "rates", "Honor gain", "Multiplier for honor points earned.", 1, 0, 20, "x", False, "world", False),
    ("Rate.Reputation.Gain", "float", "rates", "Reputation gain", "Multiplier for reputation earned.", 1, 0, 20, "x", False, "world", False),
    ("Rate.Rest.InGame", "float", "rates", "Rested bonus while playing", "Speed at which rested experience is used up while you play.", 1, 0, 10, "x", True, "world", False),
    ("Rate.Creature.Normal.Damage", "float", "advanced", "Monster damage", "Multiplier for damage dealt by normal monsters. Changes difficulty a lot.", 1, 0.1, 10, "x", True, "world", True),
    ("Rate.Creature.Normal.HP", "float", "advanced", "Monster health", "Multiplier for health of normal monsters. Changes difficulty a lot.", 1, 0.1, 10, "x", True, "world", True),
    ("PlayerLimit", "int", "world", "Maximum players", "How many players can be online at once. 0 means no limit.", 0, 0, 10000, None, False, "world", False),
    ("GameType", "enum", "advanced", "Realm type", "The realm type shown in the realm list.", 0, None, None, None, True, "world", False),
    ("RealmZone", "int", "advanced", "Realm zone", "The realm's language / region group shown to clients.", 1, 0, 30, None, True, "world", False),
    ("PlayerSaveInterval", "int", "world", "Autosave interval", "How often player characters are saved while online.", 900000, 60000, 3600000, "ms", False, "world", False),
    ("CharactersPerAccount", "int", "characters", "Characters per account", "Maximum characters an account can have across all realms.", 50, 1, 200, None, False, "world", False),
    ("CharactersPerRealm", "int", "characters", "Characters per realm", "Maximum characters an account can have on this realm.", 10, 1, 50, None, False, "world", False),
    ("StartPlayerLevel", "int", "characters", "Starting level", "Level new characters start at.", 1, 1, 80, None, False, "world", False),
    ("StartPlayerMoney", "int", "characters", "Starting gold", "Copper new characters start with (10000 copper = 1 gold).", 0, 0, 2147483647, "copper", False, "world", False),
    ("MaxPlayerLevel", "int", "characters", "Maximum level", "Highest level a character can reach. The game client only supports 80.", 80, 1, 80, None, True, "world", True),
    ("SkipCinematics", "enum", "characters", "Skip intro cinematics", "Whether new characters see their intro movie.", 0, None, None, None, False, "world", False),
    ("DurabilityLoss.OnDeath", "float", "gameplay", "Durability lost on death", "Percent of item durability lost when you die.", 10, 0, 100, "%", False, "world", False),
    ("Death.SicknessLevel", "int", "gameplay", "Resurrection sickness up to level", "Characters above this level get resurrection sickness. -10 turns it off.", 11, -10, 80, None, False, "world", False),
    ("LeaveGroupOnLogout.Enabled", "bool", "gameplay", "Leave group on logout", "Players are removed from their group when they log out.", False, None, None, None, True, "world", False),
    ("GM.LoginState", "enum", "gameplay", "GM mode at login", "Whether game masters start with GM mode on.", 2, None, None, None, True, "world", False),
    ("AllowTwoSide.Accounts", "bool", "accounts", "Both factions on one account", "One account may have Alliance and Horde characters.", True, None, None, None, False, "world", False),
    ("AllowTwoSide.Interaction.Chat", "bool", "accounts", "Cross-faction chat", "Alliance and Horde players can talk to each other.", False, None, None, None, False, "world", False),
    ("AutoBroadcast.On", "bool", "world", "Automatic announcements", "Broadcast the messages stored in the database at regular intervals.", False, None, None, None, True, "world", False),
    ("AutoBroadcast.Timer", "int", "world", "Announcement interval", "Time between automatic announcements.", 60000, 5000, 3600000, "ms", True, "world", False),
    ("Warden.Enabled", "bool", "advanced", "Anti-cheat (Warden)", "Enables the client anti-cheat check. Usually unnecessary on a private server.", True, None, None, None, True, "world", False),
    ("MapUpdateInterval", "int", "performance", "Map update interval", "How often the world updates maps. Lower is smoother but uses more CPU.", 10, 1, 1000, "ms", True, "world", False),
    ("MapUpdate.Threads", "int", "performance", "CPU cores for the world", "How many processor cores the world uses to update maps. Companions spread over several maps run much faster with more than one. Takes effect when the world server restarts.", 4, 1, 16, None, False, "world", False),
    ("MinWorldUpdateTime", "int", "performance", "Minimum world update time", "Shortest time between world updates. Raise it to reduce CPU use.", 1, 1, 1000, "ms", True, "world", False),
    ("ThreadPool", "int", "performance", "Worker threads", "Number of background worker threads. More helps large populations.", 2, 1, 32, None, True, "full", False),
    ("Network.Threads", "int", "network", "Network threads", "Threads that handle player connections. One is enough for small servers.", 1, 1, 16, None, True, "full", False),
    ("Ra.Enable", "bool", "advanced", "Remote console", "Lets the Manager and scripts send commands to the server. It only listens on this computer; keep it that way.", True, None, None, None, True, "full", True),
    ("Ra.IP", "string", "advanced", "Remote console address", "Address the remote console listens on. Keep 127.0.0.1 so it is not reachable from the internet.", "127.0.0.1", None, None, None, True, "full", True),
    ("Updates.EnableDatabases", "int", "advanced", "Automatic database updates", "Lets the server change its own databases at start. Keep 0: the Manager applies and tracks database updates safely.", 0, 0, 7, None, True, "full", True),
    ("Rate.Drop.Item.Uncommon", "float", "rates", "Uncommon item drop rate", "Multiplier for how often uncommon (green) items drop.", 1, 0, 20, "x", False, "world", False),
    ("Rate.Drop.Item.Rare", "float", "rates", "Rare item drop rate", "Multiplier for how often rare (blue) items drop.", 1, 0, 20, "x", False, "world", False),
    ("Rate.Drop.Item.Epic", "float", "rates", "Epic item drop rate", "Multiplier for how often epic (purple) items drop.", 1, 0, 20, "x", False, "world", False),
    ("Rate.Drop.Item.Legendary", "float", "rates", "Legendary item drop rate", "Multiplier for how often legendary (orange) items drop.", 1, 0, 20, "x", False, "world", False),
    ("Rate.Talent", "float", "rates", "Talent points", "Multiplier for the talent points a character earns per level.", 1, 0, 10, "x", True, "world", False),
    ("Rate.Skill.Discovery", "float", "rates", "Recipe discovery chance", "Multiplier for the chance to discover a new recipe while crafting.", 1, 0, 20, "x", True, "world", False),
    ("Rate.RepairCost", "float", "rates", "Repair cost", "Multiplier for what repairing equipment costs. 0 makes repairs free.", 1, 0, 10, "x", False, "world", False),
    ("Rate.Auction.Cut", "float", "rates", "Auction house cut", "Multiplier for the share of a sale the auction house keeps. 0 means no cut.", 1, 0, 10, "x", True, "world", False),
    ("Rate.ArenaPoints", "float", "rates", "Arena points gain", "Multiplier for arena points earned.", 1, 0, 20, "x", True, "world", False),
    ("Rate.XP.Pet", "float", "rates", "Pet experience", "Multiplier for the experience hunters' pets gain.", 1, 0, 20, "x", True, "world", False),
    ("Instance.IgnoreLevel", "bool", "gameplay", "Ignore dungeon level requirements", "Characters of any level can enter dungeons and raids.", False, None, None, None, False, "world", False),
    ("Instance.IgnoreRaid", "bool", "gameplay", "Enter raids without a raid group", "Players can enter raid instances without being in a raid group.", False, None, None, None, False, "world", False),
    ("Instance.ResetTimeHour", "int", "gameplay", "Dungeon reset hour", "Hour of the day (0-23) when dungeon and raid lockouts reset.", 4, 0, 23, None, True, "world", False),
    ("MaxPrimaryTradeSkill", "int", "gameplay", "Primary professions per character", "How many primary professions (such as Blacksmithing or Herbalism) a character may learn.", 2, 0, 11, None, False, "world", False),
    ("AlwaysMaxSkillForLevel", "bool", "gameplay", "Skills always at maximum for level", "Weapon and other skills are always at their maximum for the character's level, no training needed.", False, None, None, None, False, "world", False),
    ("ActivateWeather", "bool", "gameplay", "Weather", "Turns rain, snow and storms on or off.", True, None, None, None, False, "world", False),
    ("Group.Raid.LevelRestriction", "int", "gameplay", "Minimum level to invite to a raid", "Characters below this level cannot be invited to a raid group.", 10, 1, 80, None, True, "world", False),
    ("AllowTwoSide.Interaction.Group", "bool", "accounts", "Cross-faction groups", "Alliance and Horde players can form groups together.", False, None, None, None, False, "world", False),
    ("AllowTwoSide.Interaction.Guild", "bool", "accounts", "Cross-faction guilds", "Alliance and Horde players can join the same guild.", False, None, None, None, False, "world", False),
    ("AllowTwoSide.Interaction.Auction", "bool", "accounts", "Shared auction house", "Alliance and Horde players can use the same auction house.", False, None, None, None, False, "world", False),
    ("Achievement.RealmFirstBlockBots", "bool", "gameplay", "Bots cannot take 'Realm First!'", "Companions (bots) cannot earn 'Realm First!' achievements, so they never take a server first away from real players. Needs a server build from 2026-10-02 or later.", True, None, None, None, False, "world", False),
]
enums = {
    "GameType": [(0, "Normal"), (1, "Player vs Player"), (6, "Roleplay"), (8, "Roleplay + PvP")],
    "SkipCinematics": [(0, "Show all"), (1, "Skip for non-first characters"), (2, "Skip all")],
    "GM.LoginState": [(0, "Off"), (1, "On"), (2, "Same as last time")],
}
settings = []
for k, t, c, title, desc, d, mn, mx, unit, adv, rs, dang in R:
    s = {"key": k, "type": t, "category": c, "title": title, "description": desc, "default": d,
         "advanced": adv, "restartRequired": rs, "dangerous": dang}
    if mn is not None:
        s["min"] = mn
    if mx is not None:
        s["max"] = mx
    if unit:
        s["unit"] = unit
    if k in enums:
        s["options"] = [{"value": v, "label": l} for v, l in enums[k]]
    settings.append(s)

with open(os.path.join(ROOT, "schemas", "server.json"), "w", encoding="utf-8", newline="\n") as f:
    json.dump({"schema": 1, "scope": "server", "categories": [{"id": i, "title": t} for i, t in cats], "settings": settings}, f, indent=2, ensure_ascii=False)
    f.write("\n")

presets = [
    {"id": "vanilla", "title": "Standard rates", "description": "Everything at normal (x1) speed.",
     "values": {"Rate.XP.Kill": 1, "Rate.XP.Quest": 1, "Rate.XP.Explore": 1, "Rate.Drop.Item.Normal": 1, "Rate.Drop.Money": 1, "Rate.Honor": 1, "Rate.Reputation.Gain": 1}},
    {"id": "relaxed", "title": "Relaxed", "description": "Faster levelling and more drops for casual play (x2 / x3).",
     "values": {"Rate.XP.Kill": 2, "Rate.XP.Quest": 2, "Rate.XP.Explore": 2, "Rate.Drop.Item.Normal": 2, "Rate.Drop.Money": 3, "Rate.Honor": 2, "Rate.Reputation.Gain": 2}},
    {"id": "fast", "title": "Fast", "description": "Quick progression for testing or short sessions (x5).",
     "values": {"Rate.XP.Kill": 5, "Rate.XP.Quest": 5, "Rate.XP.Explore": 5, "Rate.Drop.Item.Normal": 3, "Rate.Drop.Money": 5, "Rate.Honor": 3, "Rate.Reputation.Gain": 3}},
]
with open(os.path.join(ROOT, "schemas", "presets", "server.json"), "w", encoding="utf-8", newline="\n") as f:
    json.dump({"schema": 1, "presets": presets}, f, indent=2)
    f.write("\n")
print(len(settings), "server settings")
