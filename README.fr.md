# CoA Server Manager

[English](README.md) · [Русский](README.ru.md) · [Deutsch](README.de.md) · **Français** · [Español](README.es.md) · [简体中文](README.zh-CN.md)

**Les personnages portables sont une fonctionnalité expérimentale.** Elle est encore en développement et peut présenter des bugs.

Une application Windows pour installer et gérer votre propre serveur **Conquest of Azeroth**, basé sur AzerothCore, en quelques étapes. Pas besoin de console, de modifier les fichiers de configuration à la main ou d'utiliser un outil de base de données. L'application utilise Tauri 2, Rust et React.

![Votre serveur en un coup d'œil](docs/screenshots/overview.png)

## Fonctionnalités

- **Personnages portables — expérimental.** Conservez vos personnages et leurs sauvegardes dans le Manager, ou rendez portable un personnage déjà présent sur un royaume auquel vous avez accès. [Voir le guide](docs/fr/PORTABLE-CHARACTERS.md).
- **Découverte des royaumes.** Comparez les royaumes publics selon leur mode de jeu, niveau maximal, taux de progression, modules et population.
- **Installation simplifiée.** Le Manager télécharge un paquet serveur signé, vérifie les fichiers, configure la base de données et crée votre premier compte. Les téléchargements peuvent reprendre après une interruption. Vous pouvez aussi importer un serveur existant.
- **Démarrer, arrêter, jouer.** Une commande suffit pour gérer la base de données et les serveurs d'authentification et de monde. L'état, la durée de fonctionnement et les joueurs connectés restent visibles. Le lancement du jeu utilise la bonne realmlist.
- **Gestion du client de jeu.** Utilisez votre client ou téléchargez-le depuis l'application : environ 43 Go, avec reprise et vérification des fichiers. Un client géré reçoit des mises à jour ; vos fichiers modifiés ne sont remplacés qu'avec votre accord.
- **Mises à jour du serveur.** Recherche au démarrage et toutes les cinq minutes, vérification des signatures Ed25519, sauvegarde préalable, conservation des modifications personnelles et possibilité de revenir en arrière.
- **Sauvegardes.** Enregistrez personnages, comptes et paramètres à la demande ou automatiquement avant les mises à jour. Vérifiez et restaurez les sauvegardes dans l'application.
- **CoA, Wildcard et Warcraft Reborn.** Choisissez le royaume dans Vue d'ensemble. La version du serveur doit prendre en charge le mode choisi. Le premier changement crée un monde distinct et une base de personnages vide. Les comptes sont partagés, mais personnages et réglages des modules restent séparés. Un serveur en cours d'exécution sauvegarde les personnages puis redémarre lors du changement. Fermez le client avant de changer de royaume ; s'il est ouvert, le choix prendra effet au prochain lancement depuis le Manager. Les bots sont indisponibles sur Wildcard et Warcraft Reborn ; les autres modules optionnels y sont expérimentaux. Sauvegardes et mises à jour couvrent tous les royaumes créés. CoA et Wildcard peuvent fonctionner ensemble. [Wildcard](docs/fr/WILDCARD.md) · [Warcraft Reborn](docs/fr/WARCRAFT-REBORN.md).
- **Bots compagnons.** Peuplez le monde avec des bots de niveau adapté, suivez leur création, mettez-la en pause ou déconnectez les bots. Retirez un nombre choisi de bots pour alléger le serveur et réglez combien se connectent au démarrage.
- **Changement de realmlist.** Enregistrez des connexions nommées, comme Solo et PTR, et choisissez-les à côté du bouton de jeu. Le fichier précédent est sauvegardé avant remplacement.
- **Comptes et réglages.** Gérez noms, mots de passe et droits d'accès. Configurez serveur et bots avec des options expliquées et des préréglages. Une console serveur et une liste des joueurs avec gestion des comptes sont également disponibles.
- **Modules.** Activez les fonctions supplémentaires, modifiez leurs réglages ou consultez leur projet GitHub depuis une même page.
- **Jouer avec des amis.** Vérifiez l'accessibilité du serveur, configurez pare-feu et UPnP, préparez une connexion en réseau local, directe ou via un réseau privé/Tailscale. Des guides et un paquet de connexion facilitent l'arrivée des amis.
- **Signalement des problèmes.** Préparez un rapport GitHub avec les versions déjà renseignées.
- **Mises à jour du Manager.** Téléchargées en arrière-plan, elles s'installent à la fermeture. Elles n'arrêtent pas vos serveurs et ne les modifient pas.
- **Langues :** anglais, russe, allemand, français, espagnol et chinois simplifié.

## Captures et guides

| Personnages portables — expérimental | Royaumes publics |
|---|---|
| ![Personnages portables](docs/screenshots/portable.png) | ![Royaumes publics](docs/screenshots/realms.png) |

| Compagnons | Sauvegardes |
|---|---|
| ![Compagnons](docs/screenshots/bots.png) | ![Sauvegardes](docs/screenshots/backups.png) |

[Galerie complète](docs/fr/SCREENSHOTS.md) · [Tous les guides](docs/fr/README.md). La galerie présente aussi l'installation, les modules, le client, les connexions entre amis et la création d'un personnage portable.

## Installation

Téléchargez et lancez l'installateur depuis les [publications GitHub](https://github.com/Corfirean/coa-server-manager/releases/latest). Prévoyez environ 10 Go libres pour le serveur. Le client de jeu est séparé et demande environ 43 Go : indiquez un dossier existant ou téléchargez-le avec le Manager.

## Historique des versions

[Changelog du Manager](CHANGELOG.md) · [Changelog du serveur](https://github.com/Corfirean/coa-server-build/blob/main/CHANGELOG.md). Leurs versions sont suivies séparément : mettre à jour le Manager ne met pas à jour le serveur.

## État du projet

**En développement actif.** Des bugs restent possibles. Si vous rencontrez un problème, [ouvrez une issue](https://github.com/Corfirean/coa-server-manager/issues/new/choose).

### Limites connues

- Le client Ascension 3.3.5 n'accepte pas les identifiants en ligne de commande. Saisissez votre mot de passe ou associez un compte existant.
- Windows protège les secrets avec DPAPI (`CryptProtectData`) et une entropie de séparation de domaine. Sous Linux/macOS, ils sont conservés dans des fichiers aux permissions `0600`.
- Une connexion TCP directe déjà établie ne peut pas basculer vers un relais en cours de session. Après une coupure, le repli automatique intervient à la connexion suivante.
- Pour importer à distance des personnages portables en sécurité, conservez `CharacterDatabase.WorkerThreads = 1`, la valeur par défaut.

## Développement

```sh
npm install
cargo test --workspace
npx tsc --noEmit
node tools/check-i18n.mjs
npx vite
cargo build -p coa-server-manager
```

Ces commandes installent les dépendances, exécutent les tests, contrôlent les types et les traductions, ouvrent l'interface dans le navigateur avec des données de test, puis compilent l'application native. En mode debug, celle-ci utilise `http://localhost:1420`.

La logique se trouve dans `crates/coa-core`, les outils de paquetage et de signature dans `crates/coa-release`, les commandes natives dans `src-tauri` et l'interface dans `src`.

`cargo run -p coa-core --example scan -- <folder>` inspecte une installation sans la modifier. Pour tester démarrage et arrêt, utilisez uniquement une copie jetable : `tools/make-fixture.ps1` crée `C:\games\coa-fixture` depuis la base intacte du paquet, avec des ports distincts. Ne testez jamais sur un serveur utilisé par des joueurs. `tests/e2e/cdp.mjs` pilote la véritable fenêtre via WebView2 avec `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222`.

## Licence

Le Manager est un logiciel libre sous [GNU AGPL v3.0](LICENSE), identifiant `AGPL-3.0-only`.

- Le serveur installé provient d'un [fork public d'AzerothCore](https://github.com/Corfirean/azerothcore-wotlk-coa). Les parties héritées de MaNGOS relèvent de GPL-2.0-or-later et les parties propres à AzerothCore d'AGPL-3.0. [mod-coa-playerbots](https://github.com/Corfirean/mod-coa-playerbots) utilise AGPL-3.0. Les commits exacts figurent dans le manifeste de chaque paquet pour retrouver les sources correspondantes.
- MySQL conserve sa licence GPL-2.0 ; son texte est fourni dans le dossier `Licenses` du paquet.
- Les données de jeu dans `Data` (`dbc`, `maps`, `vmaps`, `mmaps`) proviennent du client de l'ancien royaume Ascension Conquest of Azeroth. Elles ne sont **pas** couvertes par la licence du Manager.
