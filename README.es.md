# CoA Server Manager

[English](README.md) · [Русский](README.ru.md) · [Deutsch](README.de.md) · [Français](README.fr.md) · **Español** · [简体中文](README.zh-CN.md)

**Los personajes portátiles son una función experimental.** Sigue en desarrollo y puede contener errores.

Una aplicación para Windows con la que instalar y gestionar tu propio servidor de **Conquest of Azeroth**, basado en AzerothCore, en pocos pasos. No necesitas usar la consola, editar archivos de configuración a mano ni manejar programas de bases de datos. Creada con Tauri 2, Rust y React.

![Tu servidor de un vistazo](docs/screenshots/overview.png)

## Funciones

- **Personajes portátiles — experimental.** Guarda personajes y sus distintas versiones en el Manager, o convierte en portátil un personaje que ya tengas en un reino al que puedas acceder. [Ver la guía](docs/es/PORTABLE-CHARACTERS.md).
- **Buscador de reinos.** Compara reinos públicos por modo de juego, nivel máximo, multiplicadores, módulos y población.
- **Instalación sencilla.** Descarga un paquete de servidor firmado, comprueba los archivos, prepara la base de datos y crea tu primera cuenta. Las descargas se pueden reanudar. También puedes importar un servidor existente.
- **Iniciar, detener y jugar.** Controla conjuntamente la base de datos y los servidores de autenticación y mundo. Consulta su estado, tiempo de actividad y jugadores conectados. Al abrir el juego se utiliza la realmlist correcta.
- **Cliente de juego.** Usa el que ya tengas o descárgalo desde el Manager: unos 43 GB, con descarga reanudable y verificación de archivos. Los clientes gestionados reciben avisos de actualización; los archivos que hayas modificado solo se sustituyen con tu permiso.
- **Actualizaciones del servidor.** Se buscan al iniciar y cada cinco minutos. Los paquetes se verifican mediante firmas Ed25519, se crea una copia antes de actualizar, se conservan tus cambios y es posible volver atrás.
- **Copias de seguridad.** Guarda personajes, cuentas y ajustes cuando quieras o automáticamente antes de una actualización. Comprueba y restaura las copias desde la aplicación.
- **CoA, Wildcard y Warcraft Reborn.** Elige el reino en Resumen. La versión del servidor debe admitir el modo seleccionado. El primer cambio crea un mundo independiente y una base de personajes vacía. Las cuentas son compartidas; los personajes y los ajustes de módulos se mantienen separados. Si el servidor está encendido, guarda los personajes y se reinicia al cambiar. Cierra el cliente antes del cambio; si está abierto, el nuevo reino se aplicará la próxima vez que lo abras desde el Manager. No hay bots en Wildcard ni en Warcraft Reborn; los demás módulos opcionales son experimentales allí. Las copias y las actualizaciones cubren todos los reinos creados. CoA y Wildcard pueden funcionar a la vez. [Wildcard](docs/es/WILDCARD.md) · [Warcraft Reborn](docs/es/WARCRAFT-REBORN.md).
- **Bots compañeros.** Llena el mundo con bots de nivel adecuado, sigue su creación, ponla en pausa o desconecta los bots. Elimina una cantidad concreta para reducir la carga y decide cuántos se conectan al iniciar el servidor.
- **Cambio de realmlist.** Guarda conexiones con nombres como Solo o PTR y selecciónalas junto al botón de jugar. El archivo anterior se guarda antes de sustituirlo.
- **Cuentas y ajustes.** Gestiona nombres, contraseñas y permisos. Configura el servidor y los bots con opciones explicadas y ajustes predefinidos. También tienes una consola de servidor y una lista de jugadores con herramientas de cuentas.
- **Módulos.** Activa funciones adicionales, cambia sus ajustes o abre el repositorio de sus autores desde la misma página.
- **Jugar con amigos.** Comprueba si pueden acceder, configura el cortafuegos y UPnP, y prepara conexiones por LAN, dirección directa o red privada/Tailscale. Incluye guías y un paquete con los datos de conexión para compartir.
- **Informar de problemas.** Prepara un informe para GitHub con las versiones ya rellenadas.
- **Actualizaciones del Manager.** Se descargan en segundo plano y se instalan al cerrar la aplicación. No detienen ni modifican tus servidores.
- **Idiomas:** inglés, ruso, alemán, francés, español y chino simplificado.

## Capturas y guías

| Personajes portátiles — experimental | Reinos públicos |
|---|---|
| ![Personajes portátiles](docs/screenshots/portable.png) | ![Reinos públicos](docs/screenshots/realms.png) |

| Compañeros | Copias de seguridad |
|---|---|
| ![Compañeros](docs/screenshots/bots.png) | ![Copias de seguridad](docs/screenshots/backups.png) |

[Galería completa](docs/es/SCREENSHOTS.md) · [Todas las guías](docs/es/README.md). También encontrarás la instalación, los módulos, el cliente, las conexiones con amigos y cómo convertir un personaje existente en portátil.

## Instalación

Descarga el instalador desde la [página de versiones](https://github.com/Corfirean/coa-server-manager/releases/latest) y ejecútalo. Necesitas unos 10 GB libres para el servidor. El cliente se instala por separado y ocupa unos 43 GB: indica tu carpeta existente o descárgalo con el Manager.

## Historial de cambios

[Cambios del Manager](CHANGELOG.md) · [Cambios del servidor](https://github.com/Corfirean/coa-server-build/blob/main/CHANGELOG.md). Sus versiones se siguen por separado. Actualizar el Manager no actualiza el servidor.

## Estado del proyecto

**En desarrollo activo.** Todavía puede haber errores. Si encuentras un problema, [abre una incidencia](https://github.com/Corfirean/coa-server-manager/issues/new/choose).

### Limitaciones conocidas

- El cliente Ascension 3.3.5 no acepta credenciales por línea de comandos. Introduce la contraseña manualmente o vincula una cuenta existente.
- En Windows, los secretos se protegen con DPAPI (`CryptProtectData`) y entropía para separar dominios. Linux/macOS usan archivos con permisos `0600`.
- Una conexión TCP directa ya establecida no puede pasar a un relé a mitad de sesión. Tras una interrupción, la ruta alternativa se selecciona automáticamente en la siguiente conexión.
- Para importar personajes portátiles de forma remota con seguridad, mantén `CharacterDatabase.WorkerThreads = 1`, su valor predeterminado.

## Desarrollo

```sh
npm install
cargo test --workspace
npx tsc --noEmit
node tools/check-i18n.mjs
npx vite
cargo build -p coa-server-manager
```

Estos comandos instalan dependencias, ejecutan pruebas, comprueban tipos y traducciones, abren la interfaz en el navegador con datos de prueba y compilan la aplicación nativa. La versión de depuración utiliza `http://localhost:1420`.

La lógica está en `crates/coa-core`, las herramientas de empaquetado y firma en `crates/coa-release`, los comandos nativos en `src-tauri` y la interfaz en `src`.

`cargo run -p coa-core --example scan -- <folder>` inspecciona una instalación sin modificarla. Para probar el inicio y la parada, usa solo una copia desechable: `tools/make-fixture.ps1` crea `C:\games\coa-fixture` a partir de la base intacta del paquete y con puertos diferentes. No hagas estas pruebas en un servidor en uso. `tests/e2e/cdp.mjs` controla la ventana real mediante WebView2 con `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222`.

## Licencia

El Manager es software libre bajo [GNU AGPL v3.0](LICENSE), identificador `AGPL-3.0-only`.

- El servidor instalado procede de un [fork público de AzerothCore](https://github.com/Corfirean/azerothcore-wotlk-coa). Las partes heredadas de MaNGOS usan GPL-2.0-or-later y las propias de AzerothCore, AGPL-3.0. [mod-coa-playerbots](https://github.com/Corfirean/mod-coa-playerbots) utiliza AGPL-3.0. El manifiesto de cada paquete indica los commits exactos para localizar el código fuente correspondiente.
- MySQL conserva su licencia GPL-2.0; el texto se incluye en la carpeta `Licenses` del paquete.
- Los datos de juego de `Data` (`dbc`, `maps`, `vmaps`, `mmaps`) se extrajeron del cliente del antiguo reino Ascension Conquest of Azeroth. **No** están cubiertos por la licencia del Manager.
