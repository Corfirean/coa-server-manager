# Perfiles de reino: CoA y Wildcard

[English](../WILDCARD.md) · [Русский](../ru/WILDCARD.md) · [Deutsch](../de/WILDCARD.md) · [Français](../fr/WILDCARD.md) · **Español** · [简体中文](../zh-CN/WILDCARD.md)

Elige CoA o Wildcard en **Resumen (Overview)**. El servidor y la base deben tener la compatibilidad y las migraciones necesarias para Wildcard. Si la versión no sirve, el identificador de reino `2` está ocupado o ya existe la base de destino, el Manager se detiene para no sobrescribir datos.

El primer cambio copia el mundo CoA a `acore_world_wildcard` y crea `acore_characters_wildcard` con la estructura de tablas y los datos iniciales neutros. No copia personajes, colecciones de cartas ni progreso. Las cuentas siguen compartidas en `acore_auth`. En el funcionamiento normal, los reinos `1` y `2` usan el mismo puerto de mundo: solo funciona el seleccionado y el otro aparece desconectado. Si cambias con el servidor encendido, se detiene de forma ordenada y vuelve a arrancar con el nuevo perfil.

Los perfiles se guardan en `Settings/realm-profiles`. Si un cambio se interrumpe, el registro permite recuperar la configuración anterior. Los bots compañeros de CoA no están disponibles en Wildcard. Content Scaling y los demás módulos opcionales conservan los ajustes, pero su compatibilidad es experimental; las mecánicas de clase necesitan comprobaciones específicas dentro del juego. Volver a CoA restaura sus ajustes de módulos.

Las copias rápidas y de bases incluyen las dos bases de personajes y las cuentas comunes; las completas añaden ambos mundos. Elige el reino indicado en el punto de restauración antes de restaurar. Las actualizaciones se aplican a ambos reinos y una sola vez a las cuentas compartidas. Si se sustituye el lanzador del servidor, el Manager vuelve a instalar el adaptador de reinos antes del siguiente inicio.

![Resumen de Wildcard](../screenshots/wildcard-demo/overview.jpg)

![Compatibilidad de módulos](../screenshots/wildcard-demo/modules.jpg)

[Todas las guías](README.md) · [Iniciar ambos reinos](REPAIR_AND_REALMS.md)
