# Updates, Sicherungen und Reparatur

[English](../REPAIR_AND_REALMS.md) · [Русский](../ru/REPAIR_AND_REALMS.md) · **Deutsch** · [Français](../fr/REPAIR_AND_REALMS.md) · [Español](../es/REPAIR_AND_REALMS.md) · [简体中文](../zh-CN/REPAIR_AND_REALMS.md)

## Den Server aktualisieren

Wähle deinen Server im Manager. Sobald ein Update verfügbar ist, klicke auf **UPDATE** und folge den Anweisungen. Lass den Manager geöffnet: Er stoppt den Server, erstellt einen Wiederherstellungspunkt und prüft den aktualisierten Server.

Konten, Charaktere und Einstellungen bleiben erhalten. Updates berücksichtigen alle angelegten Realms. Falls ein Update scheitert, lies vor einem erneuten Versuch die Fehlermeldung. Die Wiederherstellungsdaten bleiben im Manager verfügbar.

## Sicherungen anlegen

Sichere vor wichtigen Änderungen unter **Sicherungen (Backups)** Charaktere, Konten und Einstellungen. Vollständige Sicherungen enthalten auch die Weltdaten. Prüfe vor dem Wiederherstellen den im Wiederherstellungspunkt angegebenen Realm und stoppe den Server.

Gespeicherte Versionen portabler Charaktere ersetzen keine Serversicherungen. **Portable Characters sind experimentell.**

## Installation prüfen und reparieren

1. Stoppe den Server.
2. Öffne **Einstellungen (Settings)** und starte **Check files and database**.
3. Lies das Ergebnis. Müssen offizielle Serverdateien wiederhergestellt werden, verwende **Repair** und folge den Anweisungen.

Die Reparatur erstellt vorher eine Sicherung und stellt offizielle Dateien wieder her. Einstellungen und Spielerdatenbanken bleiben erhalten. Möglicherweise muss die installierte Version dem aktuell veröffentlichten signierten Paket entsprechen. Verlorene Spielerdaten erfordern eine Sicherung; manche Datenbankprobleme benötigen ein korrigierendes Serverupdate.

Bleibt ein Vorgang gesperrt oder schlägt erneut fehl, nutze **Problem melden (Report a problem)** und füge Diagnose und Fehlermeldung bei. Lösche keine Datenbanken oder Wiederherstellungsprotokolle, um die Sperre zu umgehen.

## CoA und Wildcard gleichzeitig starten

Aktiviere in **Einstellungen** die Option **Start CoA and Wildcard together**. Beide Realms teilen Konten, halten Charaktere und Weltdaten aber getrennt. Beide Statusanzeigen erscheinen in der Übersicht; Stop beendet beide Realms.

War das Netzwerk für Freunde schon vorher eingerichtet, durchlaufe die Einrichtung erneut, damit auch der zweite Realm erreichbar wird. Deaktiviere den gleichzeitigen Start vor einem Wechsel zu Warcraft Reborn.

[Alle Anleitungen](README.md)
