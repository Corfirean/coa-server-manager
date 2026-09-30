"""Regenerates schemas/bots.json and schemas/presets/bots.json. Defaults mirror module/conf/mod_coa_playerbots.conf.dist."""
import json
import os

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
cats = [("general", "General"), ("population", "Population"), ("login", "Login"), ("roles", "Roles"),
        ("openworld", "Open World"), ("quests", "Quests"), ("combat", "Combat"), ("dungeon", "Dungeon Finder"),
        ("battlegrounds", "Battlegrounds"), ("social", "Social Behavior"), ("advanced", "Advanced")]
P = "CoaBots."
# key, type, category, title, description, default, min, max, unit, advanced, dangerous
R = [
    ("RandomSpawn.DefaultCount", "int", "population", "Bots created per spawn command", "How many bots the spawn command creates when you do not give a number.", 10, 1, 1000, None, False, False),
    ("RandomSpawn.MaxCount", "int", "population", "Largest single spawn", "Safety limit for one spawn command. Very large batches can freeze the server for a moment, so raise this only when nobody else is playing.", 1000, 1, 5000, None, True, True),
    ("LeveledSpawn.Brackets", "string", "population", "Level mix of new bots", "How new levelling bots are spread over levels, as ranges with weights (1-20:40 means 40 parts of bots between level 1 and 20).", "1-20:40,21-60:33,61-79:20,80-80:7", None, None, None, True, False),
    ("RandomSpawn.AutoLogin", "bool", "population", "Bring new bots online immediately", "Newly created bots log in right away instead of waiting in the database.", True, None, None, None, False, False),
    ("RandomSpawn.AccountPrefix", "string", "population", "Bot account name prefix", "Bots live on accounts that start with this name. Changing it makes existing bots stop logging in automatically.", "CoaBotHost", None, None, None, True, True),
    ("RandomSpawn.BatchSize", "int", "population", "Bots created per step", "Creation is spread over time to keep the server smooth. Higher values are faster but can cause freezes.", 5, 1, 50, None, True, True),
    ("RandomSpawn.BatchIntervalMs", "int", "population", "Pause between creation steps", "Time between creation steps. Lower values are faster but can cause freezes.", 500, 100, 60000, "ms", True, True),
    ("AutoLoginOnStartup", "bool", "login", "Automatic bot login", "Existing bots log themselves in every time the server starts.", False, None, None, None, False, False),
    ("AutoLogin.MaxCount", "int", "login", "Maximum bots online", "The most bots that log in automatically at startup.", 800, 0, 5000, None, False, False),
    ("AutoLogin.BatchSize", "int", "login", "Bots logging in per step", "Logins are spread over time. Higher values bring bots online faster but load the server harder.", 10, 1, 100, None, True, True),
    ("AutoLogin.BatchIntervalMs", "int", "login", "Pause between login steps", "Time between login steps.", 500, 100, 60000, "ms", True, True),
    ("SpecBalance.TankTargetPct", "int", "roles", "Tank population target", "Share of bots that should be tanks.", 20, 0, 50, "%", False, False),
    ("SpecBalance.HealerTargetPct", "int", "roles", "Healer population target", "Share of bots that should be healers.", 20, 0, 50, "%", False, False),
    ("TalentBuildsPath", "string", "general", "Talent data file", "Where the community talent builds are stored. Leave empty to use the built-in search.", "", None, None, None, True, False),
    ("World.Enable", "bool", "openworld", "Bots wander the world", "Idle bots run errands: visit town services, travel to gathering or grinding areas, or wander a little.", True, None, None, None, False, False),
    ("WorldBrain.Enable", "bool", "quests", "Questing AI", "Bots pick up and complete quests on their own. When off, bots only gather, fish, grind and wander.", True, None, None, None, False, False),
    ("World.ServiceRadius", "int", "openworld", "Town services search radius", "How far a bot looks for vendors and mailboxes.", 200, 50, 1000, "yards", True, False),
    ("World.AreaRadius", "int", "openworld", "Errand area radius", "How far a bot looks for gathering or grinding areas.", 350, 50, 2000, "yards", True, False),
    ("World.ThinkIntervalMs", "int", "advanced", "Errand check interval", "How often each bot reconsiders its errand.", 2000, 500, 60000, "ms", True, False),
    ("World.IdleBeforeErrandMs", "int", "advanced", "Idle time before an errand", "How long a bot must find nothing to do before it starts an errand.", 10000, 1000, 600000, "ms", True, False),
    ("World.VerboseLog", "bool", "advanced", "Detailed errand logging", "Writes every errand step to the log. Useful for debugging, noisy with many bots.", True, None, None, None, True, False),
    ("Combat.VerboseLog", "bool", "advanced", "Detailed combat logging", "Writes combat decisions to the log. Very noisy; turn on only while hunting a specific problem.", False, None, None, None, True, False),
    ("Profile.IntentMinMs", "int", "advanced", "Shortest activity", "The shortest time a bot sticks with one kind of activity.", 900000, 60000, 7200000, "ms", True, False),
    ("Profile.IntentMaxMs", "int", "advanced", "Longest activity", "The longest time a bot sticks with one kind of activity.", 2100000, 60000, 14400000, "ms", True, False),
    ("Grind.SearchRadius", "int", "openworld", "Grinding search radius", "How far a bot looks for monsters to grind.", 30, 5, 200, "yards", False, False),
    ("Grind.MaxLevelAbove", "int", "openworld", "Highest monster level to grind", "How many levels above itself a bot will fight while grinding.", 3, 0, 10, "levels", False, False),
    ("Healer.CriticalHealPct", "int", "combat", "Emergency heal threshold", "Healers switch to their strongest heals when an ally falls below this health.", 50, 5, 95, "%", False, False),
    ("WorldBrain.SearchRadius", "int", "openworld", "Target search radius", "How far a bot scans for quest targets inside an objective area.", 50, 10, 300, "yards", True, False),
    ("WorldBrain.EngageRange", "int", "combat", "Engage distance", "Distance at which a bot starts fighting a quest monster.", 28, 5, 40, "yards", True, False),
    ("WorldBrain.MountDistanceMin", "int", "openworld", "Mount up beyond (min)", "Each bot mounts for trips longer than a personal distance drawn between this and the maximum.", 60, 10, 500, "yards", True, False),
    ("WorldBrain.MountDistanceMax", "int", "openworld", "Mount up beyond (max)", "Upper end of the personal mount distance.", 95, 10, 500, "yards", True, False),
    ("WorldBrain.TaxiMinDistance", "int", "openworld", "Use flight paths beyond", "Trips longer than this use a flight path when the bot knows a route.", 900, 100, 10000, "yards", True, False),
    ("WorldBrain.SessionBreaks", "bool", "openworld", "Take breaks", "Bots alternate questing with a few minutes of town life, like a player would.", True, None, None, None, False, False),
    ("WorldBrain.GatherDetours", "bool", "openworld", "Gather nearby herbs and ore", "Gatherers step off their path for a node right next to it.", True, None, None, None, False, False),
    ("WorldBrain.MaxActiveQuests", "int", "quests", "Maximum active quests", "How many quests a bot carries at once.", 15, 1, 25, None, False, False),
    ("WorldBrain.AcceptEliteQuests", "bool", "quests", "Accept elite quests", "Solo bots take on elite quests. Off by default because they usually cannot finish them alone.", False, None, None, None, False, False),
    ("WorldBrain.QuestLevelBelow", "int", "quests", "Quests below own level", "How many levels below itself a bot still accepts quests.", 6, 0, 20, "levels", False, False),
    ("WorldBrain.QuestLevelAbove", "int", "quests", "Quests above own level", "How many levels above itself a bot still accepts quests.", 3, 0, 10, "levels", False, False),
    ("WorldBrain.MaxMobLevelAbove", "int", "quests", "Highest quest monster level", "How far above its level a quest monster may be.", 3, 0, 10, "levels", False, False),
    ("WorldBrain.MaxObjectiveDistance", "int", "quests", "Farthest quest objective", "Quests whose objectives are farther than this are not taken.", 3000, 200, 10000, "yards", True, False),
    ("WorldBrain.GiverSearchRadius", "int", "quests", "Quest giver search radius", "How far a bot looks for quest givers.", 500, 50, 3000, "yards", True, False),
    ("WorldBrain.MaxBadAreasPerObjective", "int", "quests", "Areas tried per objective", "How many areas a bot gives up on before setting the quest aside.", 3, 1, 10, None, True, False),
    ("WorldBrain.QuestSuspendMs", "int", "quests", "Set-aside time (short)", "How long a quest is set aside after the bot gives up on it.", 600000, 60000, 86400000, "ms", True, False),
    ("WorldBrain.QuestSuspendMaxMs", "int", "quests", "Set-aside time (long)", "The longest a quest can stay set aside after repeated failures.", 7200000, 60000, 86400000, "ms", True, False),
    ("LfgFill.Enable", "bool", "dungeon", "Dungeon Finder bot fill", "When you queue alone for a dungeon, bots fill the missing tank, healer and damage roles.", True, None, None, None, False, False),
    ("AutoQueueDungeon.Enable", "bool", "dungeon", "Bots queue for dungeons by themselves", "Experimental: idle tank bots queue for a dungeon on their own and the fill above completes the party.", False, None, None, None, True, True),
    ("BGFill.Enable", "bool", "battlegrounds", "Battleground bot fill", "When anyone queues for a battleground, both teams are topped up with bots so the match starts at once.", True, None, None, None, False, False),
    ("BGFill.TargetPlayersPerTeam", "int", "battlegrounds", "Battleground team size", "Bots per team to fill up to. 0 means a full team for that battleground.", 0, 0, 40, None, False, False),
    ("WorldBrain.HelpOthers", "bool", "social", "Bots help nearby players", "A passing bot joins a fight that a friendly player is losing, without taking their loot or credit.", True, None, None, None, False, False),
    ("WorldBrain.HelpRadius", "int", "social", "Help distance", "How close a bot must be to step into a fight.", 30, 5, 100, "yards", True, False),
    ("WorldBrain.HelpHealthPct", "int", "social", "Help below health", "Bots help when the fighting player is below this health.", 40, 5, 95, "%", True, False),
    ("WorldBrain.ResurrectOthers", "bool", "social", "Bots resurrect fallen players", "Bots that know a resurrection spell revive friendly corpses.", True, None, None, None, False, False),
    ("WorldBrain.TemporaryParties", "bool", "social", "Temporary bot parties", "Bots of similar level with the same quest briefly team up.", False, None, None, None, False, False),
    ("WorldBrain.PartyMaxSize", "int", "social", "Temporary party size", "Largest temporary party.", 3, 2, 5, None, True, False),
    ("WorldBrain.PartyRadius", "int", "social", "Party formation distance", "How close bots must be to team up.", 60, 10, 300, "yards", True, False),
    ("WorldBrain.PartyMinMinutes", "int", "social", "Shortest party time", "Minimum duration of a temporary party.", 5, 1, 120, "min", True, False),
    ("WorldBrain.PartyMaxMinutes", "int", "social", "Longest party time", "Maximum duration of a temporary party.", 20, 1, 240, "min", True, False),
    ("WorldBrain.PlannerMinMs", "int", "advanced", "Planner interval (min)", "Shortest time between a bot's planning passes.", 2000, 500, 60000, "ms", True, False),
    ("WorldBrain.PlannerMaxMs", "int", "advanced", "Planner interval (max)", "Longest time between a bot's planning passes.", 8000, 500, 120000, "ms", True, False),
    ("WorldBrain.ScanMinMs", "int", "advanced", "Target scan interval (min)", "Shortest time between target scans.", 500, 100, 10000, "ms", True, False),
    ("WorldBrain.ScanMaxMs", "int", "advanced", "Target scan interval (max)", "Longest time between target scans.", 1500, 100, 20000, "ms", True, False),
    ("WorldBrain.SearchTimeoutMs", "int", "advanced", "Area search time limit", "How long a bot searches an empty area before trying another.", 35000, 5000, 600000, "ms", True, False),
    ("WorldBrain.ApproachTimeoutMs", "int", "advanced", "Approach time limit", "How long a bot walks toward a claimed target.", 25000, 5000, 600000, "ms", True, False),
    ("WorldBrain.TravelTimeoutMs", "int", "advanced", "Travel time limit", "How long a whole trip to an area may take.", 360000, 30000, 3600000, "ms", True, False),
]
for n, d in [("TravelPer100Yards", 7), ("CrowdPerBot", 6), ("Overlap", 35), ("TurnIn", 120), ("Accept", 55), ("AreaOccupancy", 12), ("TargetReserved", 200)]:
    R.append(("WorldBrain.Weight." + n, "int", "advanced", "Preference weight: " + n, "How strongly bots weigh this factor when choosing what to do next. Change only if you know the planner.", d, 0, 1000, None, True, False))

settings = []
for k, t, c, title, desc, d, mn, mx, unit, adv, dang in R:
    s = {"key": P + k, "type": t, "category": c, "title": title, "description": desc, "default": d,
         "advanced": adv, "restartRequired": "world", "dangerous": dang}
    if mn is not None:
        s["min"] = mn
    if mx is not None:
        s["max"] = mx
    if unit:
        s["unit"] = unit
    settings.append(s)

with open(os.path.join(ROOT, "schemas", "bots.json"), "w", encoding="utf-8", newline="\n") as f:
    json.dump({"schema": 1, "scope": "bots", "categories": [{"id": i, "title": t} for i, t in cats], "settings": settings}, f, indent=2, ensure_ascii=False)
    f.write("\n")

quiet = {"CoaBots.World.VerboseLog": False, "CoaBots.Combat.VerboseLog": False}
presets = [
    {"id": "balanced", "title": "Balanced", "description": "The recommended everyday setup: questing, dungeon and battleground fill on, quiet logs.",
     "values": {**quiet, "CoaBots.AutoLoginOnStartup": True, "CoaBots.AutoLogin.MaxCount": 300, "CoaBots.WorldBrain.Enable": True,
                "CoaBots.LfgFill.Enable": True, "CoaBots.BGFill.Enable": True, "CoaBots.WorldBrain.TemporaryParties": False}},
    {"id": "solo-friendly", "title": "Solo Friendly", "description": "Bots that help you and fill your groups, with a small quiet population.",
     "values": {**quiet, "CoaBots.AutoLoginOnStartup": True, "CoaBots.AutoLogin.MaxCount": 100, "CoaBots.WorldBrain.HelpOthers": True,
                "CoaBots.WorldBrain.ResurrectOthers": True, "CoaBots.LfgFill.Enable": True, "CoaBots.BGFill.Enable": True,
                "CoaBots.WorldBrain.TemporaryParties": False}},
    {"id": "living-world", "title": "Living World", "description": "A busier world: more bots, breaks, gathering detours and temporary parties.",
     "values": {**quiet, "CoaBots.AutoLoginOnStartup": True, "CoaBots.AutoLogin.MaxCount": 500, "CoaBots.World.Enable": True,
                "CoaBots.WorldBrain.Enable": True, "CoaBots.WorldBrain.SessionBreaks": True, "CoaBots.WorldBrain.GatherDetours": True,
                "CoaBots.WorldBrain.TemporaryParties": True}},
    {"id": "performance", "title": "Performance", "description": "Fewer bots and less bot thinking, for weaker computers.",
     "values": {**quiet, "CoaBots.AutoLoginOnStartup": True, "CoaBots.AutoLogin.MaxCount": 50, "CoaBots.WorldBrain.TemporaryParties": False,
                "CoaBots.WorldBrain.GatherDetours": False, "CoaBots.WorldBrain.SessionBreaks": False, "CoaBots.WorldBrain.MaxActiveQuests": 8}},
]
os.makedirs(os.path.join(ROOT, "schemas", "presets"), exist_ok=True)
with open(os.path.join(ROOT, "schemas", "presets", "bots.json"), "w", encoding="utf-8", newline="\n") as f:
    json.dump({"schema": 1, "presets": presets}, f, indent=2)
    f.write("\n")
print(len(settings), "bot settings")
