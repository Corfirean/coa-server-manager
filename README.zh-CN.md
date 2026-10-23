# CoA Server Manager

[English](README.md) · [Русский](README.ru.md) · [Deutsch](README.de.md) · [Français](README.fr.md) · [Español](README.es.md) · **简体中文**

**便携角色是一项实验性功能。** 目前仍在开发中，使用时可能遇到问题。

这是一款 Windows 应用，帮助你用几个简单步骤搭建和管理自己的 **Conquest of Azeroth** 服务器，服务端基于 AzerothCore。日常操作不需要使用控制台、手动修改配置文件或单独操作数据库。应用采用 Tauri 2、Rust 和 React 开发。

![服务器概览](docs/screenshots/overview.png)

## 主要功能

- **便携角色（实验性）。** 在管理器中保存角色及其历史存档，也可以将有权访问的服务器上的现有角色转换为便携角色。[查看使用指南](docs/zh-CN/PORTABLE-CHARACTERS.md)。
- **查找服务器。** 浏览公开服务器，对比游戏模式、等级上限、倍率、模块和在线人数。
- **轻松安装。** 下载带签名的服务端安装包，逐一校验文件，配置数据库并创建首个账号。下载支持断点续传，也可以导入已有服务器。
- **启动、停止和进入游戏。** 一处控制数据库、认证服务和世界服务，随时查看运行状态、运行时间和在线玩家。启动游戏时会自动使用对应的 realmlist。
- **管理游戏客户端。** 指定现有客户端，或通过管理器下载约 43 GB 的客户端。下载可以续传，文件会经过校验。受管理的客户端可以检查更新；自行修改的文件只有在你同意后才会被替换。
- **更新服务器。** 应用启动时及之后每五分钟检查一次更新。安装包通过 Ed25519 签名验证，更新前自动备份，保留用户修改，并支持回滚。
- **备份与恢复。** 手动备份角色、账号和设置，或在更新前自动备份。在应用中即可校验和恢复。
- **CoA、Wildcard 和 Warcraft Reborn。** 在概览中选择服务器模式，服务端版本必须支持所选模式。首次切换会创建独立的世界数据库和空的角色数据库。各模式共用账号，角色和模块设置相互独立。运行中切换会先保存角色，再重启服务器。切换前请关闭客户端；如果客户端仍开着，新选择会在下次通过管理器启动游戏时生效。Wildcard 和 Warcraft Reborn 不支持机器人，其他可选模块在这两种模式下也属于实验性支持。备份和数据库更新涵盖所有已创建的模式。目前支持 CoA 与 Wildcard 同时运行。[Wildcard 指南](docs/zh-CN/WILDCARD.md) · [Warcraft Reborn 指南](docs/zh-CN/WARCRAFT-REBORN.md)。
- **伙伴机器人。** 创建等级合适的机器人并查看进度，暂停创建、让机器人下线，或移除指定数量以减轻服务器负担。也可以设置开服时登录的机器人数量。
- **切换 realmlist。** 为不同连接保存名称，例如 Solo 和 PTR，在进入游戏按钮旁直接切换。替换前会保存原文件。
- **账号和设置。** 管理账号名称、密码和权限，通过说明清楚的选项及预设调整服务器和机器人。还提供世界服务器控制台，以及带账号管理功能的玩家列表。
- **模块管理。** 在同一页面启用可选功能、调整设置，或打开相应项目的 GitHub 页面。
- **好友联机。** 检查服务器是否可达，配置防火墙和 UPnP，支持局域网、直接连接及私有网络/Tailscale。应用提供分步说明，还能准备可分享的连接资料包。
- **报告问题。** 帮你准备 GitHub 问题报告，并自动填写版本信息。
- **管理器自身更新。** 在后台下载，关闭应用时安装。更新管理器不会停止或修改你的服务器。
- **支持语言：** 英语、俄语、德语、法语、西班牙语和简体中文。

## 截图与指南

| 便携角色（实验性） | 公开服务器 |
|---|---|
| ![便携角色](docs/screenshots/portable.png) | ![公开服务器](docs/screenshots/realms.png) |

| 伙伴机器人 | 备份 |
|---|---|
| ![伙伴机器人](docs/screenshots/bots.png) | ![备份](docs/screenshots/backups.png) |

[查看完整截图](docs/zh-CN/SCREENSHOTS.md) · [玩家与服主指南](docs/zh-CN/README.md)。图库还展示了初始设置、模块、客户端、朋友联机，以及将现有角色转换为便携角色的流程。

## 安装

从 [GitHub 发布页](https://github.com/Corfirean/coa-server-manager/releases/latest)下载安装程序并运行。服务器约需 10 GB 可用空间。游戏客户端单独安装，约占 43 GB；你可以指定已有客户端目录，也可以交给管理器下载。

## 更新记录

[管理器更新记录](CHANGELOG.md) · [服务器更新记录](https://github.com/Corfirean/coa-server-build/blob/main/CHANGELOG.md)。两者的版本和更新记录分别维护。更新管理器不会同时更新服务器。

## 项目状态

**正在积极开发中。** 仍可能存在问题。如果遇到故障，请[提交 issue](https://github.com/Corfirean/coa-server-manager/issues/new/choose)。

### 已知限制

- Ascension 3.3.5 客户端不支持通过命令行传入账号和密码。请自行输入密码，或关联现有账号。
- Windows 使用 DPAPI（`CryptProtectData`）及用于域隔离的额外熵来保护机密数据。Linux/macOS 使用权限为 `0600` 的文件。
- 已建立的直连 TCP 会话无法在不中断连接的情况下切换至中继。连接中断后，会在下一次加入服务器时自动尝试备用路径。
- 为保证远程导入便携角色的安全，请保留默认设置 `CharacterDatabase.WorkerThreads = 1`。

## 开发

```sh
npm install
cargo test --workspace
npx tsc --noEmit
node tools/check-i18n.mjs
npx vite
cargo build -p coa-server-manager
```

以上命令依次安装依赖、运行测试、检查类型与翻译、启动使用模拟数据的浏览器界面，以及编译原生应用。调试版应用连接 `http://localhost:1420`。

`crates/coa-core` 存放核心逻辑，`crates/coa-release` 提供打包和签名工具，`src-tauri` 处理原生命令，`src` 存放界面代码。

`cargo run -p coa-core --example scan -- <folder>` 可以只读检查现有安装。启动和停止测试必须使用可丢弃的独立副本：`tools/make-fixture.ps1` 会从安装包的原始数据库创建 `C:\games\coa-fixture`，并使用不同端口。不要在正在供玩家使用的服务器上进行此类测试。`tests/e2e/cdp.mjs` 可通过 WebView2 控制真实窗口，需设置 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222`。

## 许可

管理器是自由软件，采用 [GNU AGPL v3.0](LICENSE)，标识为 `AGPL-3.0-only`。

- 所安装的服务器来自公开的 [AzerothCore 分支](https://github.com/Corfirean/azerothcore-wotlk-coa)。继承自 MaNGOS 的部分采用 GPL-2.0-or-later，AzerothCore 自有部分采用 AGPL-3.0。[mod-coa-playerbots](https://github.com/Corfirean/mod-coa-playerbots) 采用 AGPL-3.0。每个安装包的清单都记录了准确的提交版本，便于查找对应源码。
- 随附的 MySQL 保留其 GPL-2.0 许可，许可文本位于安装包的 `Licenses` 目录。
- `Data` 中的游戏数据（`dbc`、`maps`、`vmaps`、`mmaps`）提取自已关闭的 Ascension Conquest of Azeroth 服务器客户端，**不属于**管理器许可的授权范围。
