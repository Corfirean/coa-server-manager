"""One-off: appends 20 more curated worldserver.conf settings to tools/gen_server_schema.py and their translations to
src/i18n/schema/*.json. Run once, then run gen_server_schema.py and tools/check-i18n.mjs."""
import json
import os

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")

# key, type, category, title, description, default, min, max, unit, advanced, restart, dangerous
NEW = [
    ("Rate.Drop.Item.Uncommon", "float", "rates", "Uncommon item drop rate", "Multiplier for how often uncommon (green) items drop.", 1, 0, 20, "x", False, "world", False),
    ("Rate.Drop.Item.Rare", "float", "rates", "Rare item drop rate", "Multiplier for how often rare (blue) items drop.", 1, 0, 20, "x", False, "world", False),
    ("Rate.Drop.Item.Epic", "float", "rates", "Epic item drop rate", "Multiplier for how often epic (purple) items drop.", 1, 0, 20, "x", False, "world", False),
    ("Rate.Drop.Item.Legendary", "float", "rates", "Legendary item drop rate", "Multiplier for how often legendary (orange) items drop.", 1, 0, 20, "x", False, "world", False),
    ("Rate.Talent", "float", "rates", "Standard talent points (AzerothCore)", "Only affects standard AzerothCore talents. CoA class and specialization points come from client data and ignore this multiplier.", 1, 0, 10, "x", True, "world", False),
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
]

# language -> key -> (title, description)
T = {
    "ru": {
        "Rate.Drop.Item.Uncommon": ("Шанс выпадения необычных предметов", "Множитель частоты выпадения необычных (зелёных) предметов."),
        "Rate.Drop.Item.Rare": ("Шанс выпадения редких предметов", "Множитель частоты выпадения редких (синих) предметов."),
        "Rate.Drop.Item.Epic": ("Шанс выпадения эпических предметов", "Множитель частоты выпадения эпических (фиолетовых) предметов."),
        "Rate.Drop.Item.Legendary": ("Шанс выпадения легендарных предметов", "Множитель частоты выпадения легендарных (оранжевых) предметов."),
        "Rate.Talent": ("Обычные очки талантов (AzerothCore)", "Влияет только на обычные таланты AzerothCore. Очки классового дерева и специализации CoA берутся из данных клиента и не зависят от этого множителя."),
        "Rate.Skill.Discovery": ("Шанс открытия рецептов", "Множитель шанса открыть новый рецепт при создании предметов."),
        "Rate.RepairCost": ("Стоимость ремонта", "Множитель стоимости ремонта снаряжения. 0 делает ремонт бесплатным."),
        "Rate.Auction.Cut": ("Комиссия аукциона", "Множитель доли от продажи, которую забирает аукцион. 0 — без комиссии."),
        "Rate.ArenaPoints": ("Очки арены", "Множитель получаемых очков арены."),
        "Rate.XP.Pet": ("Опыт питомцев", "Множитель опыта, который получают питомцы охотников."),
        "Instance.IgnoreLevel": ("Игнорировать требования уровня для подземелий", "Персонажи любого уровня могут входить в подземелья и рейды."),
        "Instance.IgnoreRaid": ("Вход в рейды без рейдовой группы", "Игроки могут входить в рейдовые подземелья, не состоя в рейдовой группе."),
        "Instance.ResetTimeHour": ("Час сброса подземелий", "Час суток (0–23), когда сбрасываются привязки к подземельям и рейдам."),
        "MaxPrimaryTradeSkill": ("Основных профессий на персонажа", "Сколько основных профессий (например, кузнечное дело или травничество) может изучить персонаж."),
        "AlwaysMaxSkillForLevel": ("Навыки всегда на максимуме для уровня", "Навыки оружия и другие навыки всегда на максимуме для уровня персонажа, учиться не нужно."),
        "ActivateWeather": ("Погода", "Включает или выключает дождь, снег и бури."),
        "Group.Raid.LevelRestriction": ("Минимальный уровень для приглашения в рейд", "Персонажей ниже этого уровня нельзя пригласить в рейдовую группу."),
        "AllowTwoSide.Interaction.Group": ("Группы между фракциями", "Игроки Альянса и Орды могут создавать общие группы."),
        "AllowTwoSide.Interaction.Guild": ("Гильдии между фракциями", "Игроки Альянса и Орды могут состоять в одной гильдии."),
        "AllowTwoSide.Interaction.Auction": ("Общий аукцион", "Игроки Альянса и Орды пользуются одним аукционом."),
    },
    "de": {
        "Rate.Drop.Item.Uncommon": ("Droprate für ungewöhnliche Gegenstände", "Multiplikator dafür, wie oft ungewöhnliche (grüne) Gegenstände fallen."),
        "Rate.Drop.Item.Rare": ("Droprate für seltene Gegenstände", "Multiplikator dafür, wie oft seltene (blaue) Gegenstände fallen."),
        "Rate.Drop.Item.Epic": ("Droprate für epische Gegenstände", "Multiplikator dafür, wie oft epische (lila) Gegenstände fallen."),
        "Rate.Drop.Item.Legendary": ("Droprate für legendäre Gegenstände", "Multiplikator dafür, wie oft legendäre (orange) Gegenstände fallen."),
        "Rate.Talent": ("Standard-Talentpunkte (AzerothCore)", "Gilt nur für Standardtalente von AzerothCore. CoA-Klassen- und Spezialisierungspunkte stammen aus Clientdaten und ignorieren diesen Multiplikator."),
        "Rate.Skill.Discovery": ("Chance auf Rezeptentdeckung", "Multiplikator für die Chance, beim Herstellen ein neues Rezept zu entdecken."),
        "Rate.RepairCost": ("Reparaturkosten", "Multiplikator für die Kosten der Ausrüstungsreparatur. 0 macht Reparaturen kostenlos."),
        "Rate.Auction.Cut": ("Auktionshaus-Gebühr", "Multiplikator für den Anteil eines Verkaufs, den das Auktionshaus behält. 0 bedeutet keine Gebühr."),
        "Rate.ArenaPoints": ("Arenapunkte", "Multiplikator für erhaltene Arenapunkte."),
        "Rate.XP.Pet": ("Begleiter-Erfahrung", "Multiplikator für die Erfahrung, die Begleiter von Jägern erhalten."),
        "Instance.IgnoreLevel": ("Stufenanforderungen für Dungeons ignorieren", "Charaktere jeder Stufe können Dungeons und Schlachtzüge betreten."),
        "Instance.IgnoreRaid": ("Schlachtzüge ohne Schlachtzugsgruppe betreten", "Spieler können Schlachtzugsinstanzen betreten, ohne in einer Schlachtzugsgruppe zu sein."),
        "Instance.ResetTimeHour": ("Dungeon-Reset-Stunde", "Uhrzeit (0–23), zu der Dungeon- und Schlachtzugssperren zurückgesetzt werden."),
        "MaxPrimaryTradeSkill": ("Hauptberufe pro Charakter", "Wie viele Hauptberufe (z. B. Schmiedekunst oder Kräuterkunde) ein Charakter erlernen darf."),
        "AlwaysMaxSkillForLevel": ("Fertigkeiten immer auf Maximum der Stufe", "Waffen- und andere Fertigkeiten sind immer auf dem Maximum der Charakterstufe, Training ist nicht nötig."),
        "ActivateWeather": ("Wetter", "Schaltet Regen, Schnee und Stürme ein oder aus."),
        "Group.Raid.LevelRestriction": ("Mindeststufe zum Einladen in einen Schlachtzug", "Charaktere unter dieser Stufe können nicht in eine Schlachtzugsgruppe eingeladen werden."),
        "AllowTwoSide.Interaction.Group": ("Gruppen über Fraktionen hinweg", "Spieler der Allianz und der Horde können gemeinsam Gruppen bilden."),
        "AllowTwoSide.Interaction.Guild": ("Gilden über Fraktionen hinweg", "Spieler der Allianz und der Horde können derselben Gilde beitreten."),
        "AllowTwoSide.Interaction.Auction": ("Gemeinsames Auktionshaus", "Spieler der Allianz und der Horde nutzen dasselbe Auktionshaus."),
    },
    "fr": {
        "Rate.Drop.Item.Uncommon": ("Taux de butin des objets inhabituels", "Multiplicateur de la fréquence de butin des objets inhabituels (verts)."),
        "Rate.Drop.Item.Rare": ("Taux de butin des objets rares", "Multiplicateur de la fréquence de butin des objets rares (bleus)."),
        "Rate.Drop.Item.Epic": ("Taux de butin des objets épiques", "Multiplicateur de la fréquence de butin des objets épiques (violets)."),
        "Rate.Drop.Item.Legendary": ("Taux de butin des objets légendaires", "Multiplicateur de la fréquence de butin des objets légendaires (orange)."),
        "Rate.Talent": ("Points de talent standards (AzerothCore)", "Affecte uniquement les talents standards AzerothCore. Les points de classe et de spécialisation CoA proviennent des données du client et ignorent ce multiplicateur."),
        "Rate.Skill.Discovery": ("Chance de découvrir des recettes", "Multiplicateur de la chance de découvrir une nouvelle recette en fabriquant."),
        "Rate.RepairCost": ("Coût des réparations", "Multiplicateur du coût de réparation de l'équipement. 0 rend les réparations gratuites."),
        "Rate.Auction.Cut": ("Commission de l'hôtel des ventes", "Multiplicateur de la part d'une vente que garde l'hôtel des ventes. 0 signifie aucune commission."),
        "Rate.ArenaPoints": ("Points d'arène", "Multiplicateur des points d'arène gagnés."),
        "Rate.XP.Pet": ("Expérience des familiers", "Multiplicateur de l'expérience gagnée par les familiers des chasseurs."),
        "Instance.IgnoreLevel": ("Ignorer les niveaux requis des donjons", "Les personnages de tout niveau peuvent entrer dans les donjons et les raids."),
        "Instance.IgnoreRaid": ("Entrer dans les raids sans groupe de raid", "Les joueurs peuvent entrer dans les instances de raid sans être dans un groupe de raid."),
        "Instance.ResetTimeHour": ("Heure de réinitialisation des donjons", "Heure de la journée (0-23) à laquelle les verrouillages de donjons et de raids sont réinitialisés."),
        "MaxPrimaryTradeSkill": ("Métiers principaux par personnage", "Nombre de métiers principaux (comme la forge ou l'herboristerie) qu'un personnage peut apprendre."),
        "AlwaysMaxSkillForLevel": ("Compétences toujours au maximum du niveau", "Les compétences d'armes et autres sont toujours au maximum du niveau du personnage, sans entraînement."),
        "ActivateWeather": ("Météo", "Active ou désactive la pluie, la neige et les tempêtes."),
        "Group.Raid.LevelRestriction": ("Niveau minimum pour inviter dans un raid", "Les personnages en dessous de ce niveau ne peuvent pas être invités dans un groupe de raid."),
        "AllowTwoSide.Interaction.Group": ("Groupes entre factions", "Les joueurs de l'Alliance et de la Horde peuvent former des groupes ensemble."),
        "AllowTwoSide.Interaction.Guild": ("Guildes entre factions", "Les joueurs de l'Alliance et de la Horde peuvent rejoindre la même guilde."),
        "AllowTwoSide.Interaction.Auction": ("Hôtel des ventes commun", "Les joueurs de l'Alliance et de la Horde utilisent le même hôtel des ventes."),
    },
    "es": {
        "Rate.Drop.Item.Uncommon": ("Probabilidad de botín de objetos poco comunes", "Multiplicador de la frecuencia con que caen objetos poco comunes (verdes)."),
        "Rate.Drop.Item.Rare": ("Probabilidad de botín de objetos raros", "Multiplicador de la frecuencia con que caen objetos raros (azules)."),
        "Rate.Drop.Item.Epic": ("Probabilidad de botín de objetos épicos", "Multiplicador de la frecuencia con que caen objetos épicos (morados)."),
        "Rate.Drop.Item.Legendary": ("Probabilidad de botín de objetos legendarios", "Multiplicador de la frecuencia con que caen objetos legendarios (naranjas)."),
        "Rate.Talent": ("Puntos de talento estándar (AzerothCore)", "Solo afecta a los talentos estándar de AzerothCore. Los puntos de clase y especialización de CoA proceden de los datos del cliente e ignoran este multiplicador."),
        "Rate.Skill.Discovery": ("Probabilidad de descubrir recetas", "Multiplicador de la probabilidad de descubrir una receta nueva al fabricar."),
        "Rate.RepairCost": ("Coste de reparación", "Multiplicador del coste de reparar el equipo. 0 hace las reparaciones gratuitas."),
        "Rate.Auction.Cut": ("Comisión de la casa de subastas", "Multiplicador de la parte de una venta que se queda la casa de subastas. 0 significa sin comisión."),
        "Rate.ArenaPoints": ("Puntos de arena", "Multiplicador de los puntos de arena obtenidos."),
        "Rate.XP.Pet": ("Experiencia de mascotas", "Multiplicador de la experiencia que ganan las mascotas de los cazadores."),
        "Instance.IgnoreLevel": ("Ignorar el nivel requerido de las mazmorras", "Personajes de cualquier nivel pueden entrar en mazmorras y bandas."),
        "Instance.IgnoreRaid": ("Entrar en bandas sin grupo de banda", "Los jugadores pueden entrar en instancias de banda sin estar en un grupo de banda."),
        "Instance.ResetTimeHour": ("Hora de reinicio de mazmorras", "Hora del día (0-23) a la que se reinician los bloqueos de mazmorras y bandas."),
        "MaxPrimaryTradeSkill": ("Profesiones principales por personaje", "Cuántas profesiones principales (como herrería o herboristería) puede aprender un personaje."),
        "AlwaysMaxSkillForLevel": ("Habilidades siempre al máximo del nivel", "Las habilidades de armas y otras están siempre al máximo para el nivel del personaje, sin entrenar."),
        "ActivateWeather": ("Clima", "Activa o desactiva la lluvia, la nieve y las tormentas."),
        "Group.Raid.LevelRestriction": ("Nivel mínimo para invitar a una banda", "Los personajes por debajo de este nivel no pueden ser invitados a un grupo de banda."),
        "AllowTwoSide.Interaction.Group": ("Grupos entre facciones", "Jugadores de la Alianza y de la Horda pueden formar grupos juntos."),
        "AllowTwoSide.Interaction.Guild": ("Hermandades entre facciones", "Jugadores de la Alianza y de la Horda pueden unirse a la misma hermandad."),
        "AllowTwoSide.Interaction.Auction": ("Casa de subastas compartida", "Jugadores de la Alianza y de la Horda usan la misma casa de subastas."),
    },
}


def fmt(v):
    if isinstance(v, bool):
        return "True" if v else "False"
    return repr(v) if isinstance(v, str) else str(v)


def main():
    path = os.path.join(ROOT, "tools", "gen_server_schema.py")
    src = open(path, encoding="utf-8").read()
    if "Rate.Drop.Item.Uncommon" in src:
        print("generator already updated")
    else:
        lines = []
        for k, t, c, title, desc, d, mn, mx, unit, adv, rs, dang in NEW:
            lines.append("    (%s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s, %s),\n" % (
                json.dumps(k), json.dumps(t), json.dumps(c), json.dumps(title), json.dumps(desc), fmt(d),
                fmt(mn) if mn is not None else "None", fmt(mx) if mx is not None else "None",
                json.dumps(unit) if unit else "None", fmt(adv), json.dumps(rs), fmt(dang)))
        marker = "]\nenums = {"
        assert marker in src
        src = src.replace(marker, "".join(lines) + marker, 1)
        open(path, "w", encoding="utf-8", newline="\n").write(src)
    for lang, tr in T.items():
        p = os.path.join(ROOT, "src", "i18n", "schema", lang + ".json")
        data = json.load(open(p, encoding="utf-8"))
        for k, (t, d) in tr.items():
            data["settings"][k] = {"t": t, "d": d}
        with open(p, "w", encoding="utf-8", newline="\n") as f:
            json.dump(data, f, indent=1, ensure_ascii=False)
            f.write("\n")


main()
