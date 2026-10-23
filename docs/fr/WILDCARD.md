# Profils de royaume : CoA et Wildcard

[English](../WILDCARD.md) · [Русский](../ru/WILDCARD.md) · [Deutsch](../de/WILDCARD.md) · **Français** · [Español](../es/WILDCARD.md) · [简体中文](../zh-CN/WILDCARD.md)

Choisissez CoA ou Wildcard dans **Vue d'ensemble (Overview)**. Le serveur et sa base doivent disposer de la prise en charge de Wildcard. Le Manager refuse une version incompatible, un identifiant de royaume `2` déjà occupé ou une base de destination existante afin de ne rien écraser.

Au premier changement, le monde CoA est copié dans `acore_world_wildcard`. `acore_characters_wildcard` reçoit la structure des tables et les données initiales neutres, sans copier personnages, collections de cartes ou progression. Les comptes restent partagés dans `acore_auth`. En fonctionnement normal, les royaumes `1` et `2` utilisent le même port de monde : seul le royaume choisi fonctionne, l'autre est affiché hors ligne. Un changement en cours d'exécution arrête proprement le serveur puis le redémarre avec le nouveau profil.

Les réglages sont enregistrés dans `Settings/realm-profiles`. Si le changement est interrompu, le journal permet de retrouver la configuration précédente. Les bots compagnons CoA ne sont pas disponibles sur Wildcard. Content Scaling et les autres modules gardent leurs réglages mais leur compatibilité y est expérimentale ; les mécaniques de classe demandent des vérifications en jeu distinctes. Revenir à CoA restaure ses réglages de modules.

Les sauvegardes rapides et de bases incluent les deux bases de personnages et les comptes communs ; les complètes ajoutent les deux mondes. Sélectionnez le royaume indiqué sur le point de restauration avant de restaurer. Les mises à jour s'appliquent aux deux royaumes, et une seule fois aux comptes communs. Si le lanceur serveur est remplacé, le Manager réinstalle son adaptateur de royaumes avant le prochain démarrage.

![Vue d'ensemble de Wildcard](../screenshots/wildcard-demo/overview.jpg)

![Compatibilité des modules](../screenshots/wildcard-demo/modules.jpg)

[Tous les guides](README.md) · [Lancer les deux royaumes ensemble](REPAIR_AND_REALMS.md)
