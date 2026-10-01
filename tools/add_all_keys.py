"""One-off: appends the 'all settings' UI strings to every locale file."""
import os

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
K = {
    "en": {
        "all.title": "All server settings",
        "all.intro": "Every documented setting of the world server, with its explanation. Change only what you understand: a wrong value can break the server. The previous configuration is saved first and can be restored.",
        "all.search": "Search by name or description…",
        "all.onlyChanged": "Only changed",
        "all.count": "Showing {shown} of {total}. Search to narrow it down.",
        "all.none": "No settings match.",
        "all.changed": "changed",
        "all.useDefault": "Use the default ({v})",
        "all.save": "Save {n} change(s)",
        "all.saved": "Saved. Restart the server to apply the changes.",
    },
    "ru": {
        "all.title": "Все настройки сервера",
        "all.intro": "Все документированные настройки мирового сервера с пояснениями. Меняйте только то, что понимаете: неверное значение может сломать сервер. Прежняя конфигурация сохраняется заранее, её можно восстановить.",
        "all.search": "Поиск по названию или описанию…",
        "all.onlyChanged": "Только изменённые",
        "all.count": "Показано {shown} из {total}. Используйте поиск, чтобы сузить список.",
        "all.none": "Ничего не найдено.",
        "all.changed": "изменено",
        "all.useDefault": "Вернуть значение по умолчанию ({v})",
        "all.save": "Сохранить изменения: {n}",
        "all.saved": "Сохранено. Перезапустите сервер, чтобы изменения вступили в силу.",
    },
    "de": {
        "all.title": "Alle Servereinstellungen",
        "all.intro": "Jede dokumentierte Einstellung des Weltservers mit Erklärung. Ändere nur, was du verstehst: Ein falscher Wert kann den Server beschädigen. Die bisherige Konfiguration wird vorher gesichert und kann wiederhergestellt werden.",
        "all.search": "Nach Name oder Beschreibung suchen…",
        "all.onlyChanged": "Nur geänderte",
        "all.count": "{shown} von {total} werden angezeigt. Suche, um die Liste einzugrenzen.",
        "all.none": "Keine Einstellungen gefunden.",
        "all.changed": "geändert",
        "all.useDefault": "Standardwert verwenden ({v})",
        "all.save": "{n} Änderung(en) speichern",
        "all.saved": "Gespeichert. Starte den Server neu, um die Änderungen zu übernehmen.",
    },
    "fr": {
        "all.title": "Tous les réglages du serveur",
        "all.intro": "Chaque réglage documenté du serveur de monde, avec son explication. Ne modifiez que ce que vous comprenez : une mauvaise valeur peut casser le serveur. La configuration précédente est sauvegardée d'abord et peut être restaurée.",
        "all.search": "Rechercher par nom ou description…",
        "all.onlyChanged": "Seulement les modifiés",
        "all.count": "{shown} sur {total} affichés. Utilisez la recherche pour affiner.",
        "all.none": "Aucun réglage ne correspond.",
        "all.changed": "modifié",
        "all.useDefault": "Utiliser la valeur par défaut ({v})",
        "all.save": "Enregistrer {n} modification(s)",
        "all.saved": "Enregistré. Redémarrez le serveur pour appliquer les changements.",
    },
    "es": {
        "all.title": "Todos los ajustes del servidor",
        "all.intro": "Cada ajuste documentado del servidor del mundo, con su explicación. Cambia solo lo que entiendas: un valor incorrecto puede romper el servidor. La configuración anterior se guarda antes y se puede restaurar.",
        "all.search": "Buscar por nombre o descripción…",
        "all.onlyChanged": "Solo los cambiados",
        "all.count": "Mostrando {shown} de {total}. Usa la búsqueda para acotar.",
        "all.none": "Ningún ajuste coincide.",
        "all.changed": "cambiado",
        "all.useDefault": "Usar el valor predeterminado ({v})",
        "all.save": "Guardar {n} cambio(s)",
        "all.saved": "Guardado. Reinicia el servidor para aplicar los cambios.",
    },
}

for lang, keys in K.items():
    p = os.path.join(ROOT, "src", "i18n", "locales", lang + ".ts")
    s = open(p, encoding="utf-8").read()
    if '"all.title"' in s:
        continue
    lines = s.split("\n")
    idx = max(i for i, l in enumerate(lines) if l.startswith("}"))
    add = ["  // Settings page: the full searchable list"]
    for k, v in keys.items():
        add.append('  %s: %s,' % ('"' + k + '"', '"' + v.replace("\\", "\\\\").replace('"', '\\"') + '"'))
    lines[idx:idx] = add
    open(p, "w", encoding="utf-8", newline="\n").write("\n".join(lines))
