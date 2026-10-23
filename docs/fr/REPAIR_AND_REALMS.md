# Mises à jour, sauvegardes et réparation

[English](../REPAIR_AND_REALMS.md) · [Русский](../ru/REPAIR_AND_REALMS.md) · [Deutsch](../de/REPAIR_AND_REALMS.md) · **Français** · [Español](../es/REPAIR_AND_REALMS.md) · [简体中文](../zh-CN/REPAIR_AND_REALMS.md)

## Mettre le serveur à jour

Sélectionnez votre serveur dans le Manager. Lorsqu'une mise à jour est proposée, cliquez sur **UPDATE** et suivez les instructions. Gardez l'application ouverte jusqu'à la fin : elle arrête le serveur, crée un point de restauration et vérifie le serveur mis à jour.

Comptes, personnages et réglages sont conservés. Tous les royaumes créés sont concernés. En cas d'échec, lisez le message avant de réessayer ; les informations de restauration restent disponibles dans le Manager.

## Prévoir une sauvegarde

Avant une modification importante, utilisez **Sauvegardes (Backups)** pour conserver personnages, comptes et paramètres. Une sauvegarde complète contient aussi les données du monde. Vérifiez le royaume indiqué sur le point de restauration et arrêtez le serveur avant de restaurer.

Les versions sauvegardées d'un personnage portable ne remplacent pas les sauvegardes du serveur. **Les personnages portables sont expérimentaux.**

## Vérifier ou réparer l'installation

1. Arrêtez le serveur.
2. Dans **Paramètres (Settings)**, lancez **Check files and database**.
3. Consultez les résultats. Si des fichiers officiels doivent être restaurés, choisissez **Repair** et suivez les instructions.

La réparation crée une sauvegarde et restaure les fichiers officiels en préservant paramètres et bases des joueurs. Elle peut exiger une version installée identique au paquet signé actuellement publié. Les données de joueurs perdues nécessitent une sauvegarde ; certains problèmes de base demandent une mise à jour corrective du serveur.

Si l'opération reste bloquée ou échoue de nouveau, utilisez **Signaler un problème (Report a problem)** et joignez diagnostic et message d'erreur. Ne supprimez pas les bases ou les journaux de restauration pour contourner un blocage.

## Faire fonctionner CoA et Wildcard ensemble

Dans **Paramètres**, activez **Start CoA and Wildcard together**. Les comptes sont communs, mais personnages et mondes restent séparés. Les deux états apparaissent dans Vue d'ensemble et l'arrêt termine les deux royaumes.

Si vous aviez configuré le réseau pour vos amis auparavant, refaites cette configuration pour rendre aussi le second royaume accessible. Désactivez le démarrage simultané avant de passer à Warcraft Reborn.

[Tous les guides](README.md)
