"""One-off: import-screen guidance for a wrongly chosen folder (all locales)."""
import os, re
ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
NEW = {
 "en": {"import.incompatible.text": "Choose the main server folder: the one that contains the folders Core, Data, mysql, Scripts and Settings (not the Core folder itself).",
        "import.suggest.title": "Did you mean this folder?", "import.suggest.text": "The folder you chose belongs to a server. The server's main folder is:", "import.suggest.use": "Use this folder",
        "import.notRepack": "This looks like a standalone AzerothCore server. The Manager works with the CoA Repack layout (a folder that contains Core, mysql, Scripts and Settings). Use \"Install a new server\" instead, or import a CoA Repack."},
 "ru": {"import.incompatible.text": "Выберите главную папку сервера: ту, где лежат папки Core, Data, mysql, Scripts и Settings (а не саму папку Core).",
        "import.suggest.title": "Возможно, вы имели в виду эту папку?", "import.suggest.text": "Выбранная папка является частью сервера. Главная папка сервера:", "import.suggest.use": "Использовать эту папку",
        "import.notRepack": "Похоже на отдельный сервер AzerothCore. Менеджер работает со структурой CoA Repack (папка, где лежат Core, mysql, Scripts и Settings). Выберите «Установить новый сервер» или подключите CoA Repack."},
 "de": {"import.incompatible.text": "Wähle den Hauptordner des Servers: den, der die Ordner Core, Data, mysql, Scripts und Settings enthält (nicht den Ordner Core selbst).",
        "import.suggest.title": "Meintest du diesen Ordner?", "import.suggest.text": "Der gewählte Ordner gehört zu einem Server. Der Hauptordner des Servers ist:", "import.suggest.use": "Diesen Ordner verwenden",
        "import.notRepack": "Das sieht nach einem eigenständigen AzerothCore-Server aus. Der Manager arbeitet mit dem Aufbau des CoA Repack (ein Ordner mit Core, mysql, Scripts und Settings). Nutze stattdessen „Neuen Server installieren“ oder binde ein CoA Repack ein."},
 "fr": {"import.incompatible.text": "Choisissez le dossier principal du serveur : celui qui contient les dossiers Core, Data, mysql, Scripts et Settings (et non le dossier Core lui-même).",
        "import.suggest.title": "Vouliez-vous ce dossier ?", "import.suggest.text": "Le dossier choisi fait partie d'un serveur. Le dossier principal du serveur est :", "import.suggest.use": "Utiliser ce dossier",
        "import.notRepack": "Cela ressemble à un serveur AzerothCore autonome. Le Manager fonctionne avec la structure du CoA Repack (un dossier contenant Core, mysql, Scripts et Settings). Utilisez plutôt « Installer un nouveau serveur » ou importez un CoA Repack."},
 "es": {"import.incompatible.text": "Elige la carpeta principal del servidor: la que contiene las carpetas Core, Data, mysql, Scripts y Settings (no la carpeta Core en sí).",
        "import.suggest.title": "¿Quisiste decir esta carpeta?", "import.suggest.text": "La carpeta elegida pertenece a un servidor. La carpeta principal del servidor es:", "import.suggest.use": "Usar esta carpeta",
        "import.notRepack": "Parece un servidor AzerothCore independiente. El Manager funciona con la estructura del CoA Repack (una carpeta que contiene Core, mysql, Scripts y Settings). Usa «Instalar un servidor nuevo» o importa un CoA Repack."},
}
def q(v): return '"' + v.replace("\\", "\\\\").replace('"', '\\"') + '"'
for lang, keys in NEW.items():
    p = os.path.join(ROOT, "src", "i18n", "locales", lang + ".ts")
    s = open(p, encoding="utf-8").read()
    for k, v in keys.items():
        pat = re.compile(r'^(\s*)"%s": .*,$' % re.escape(k), re.M)
        if pat.search(s):
            s = pat.sub(lambda m: '%s"%s": %s,' % (m.group(1), k, q(v)), s, count=1)
        else:
            lines = s.split("\n")
            idx = max(i for i, l in enumerate(lines) if l.startswith("}"))
            lines.insert(idx, '  "%s": %s,' % (k, q(v)))
            s = "\n".join(lines)
    open(p, "w", encoding="utf-8", newline="\n").write(s)
