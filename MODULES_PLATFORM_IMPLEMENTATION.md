# Отчёт о реализации платформы модулей CoA и автономной редакции CSM Modules Preview

**Дата:** 2 ноября 2026 г.  
**Ветка:** `preview/modules`  
**Коммит:** `2c37c4e`  
**Исполняемый файл Preview:** `C:\games\coa-server-manager-preview\target\release\csm-modules-preview.exe` (32.6 МБ, PE x64)  
**Портативный дистрибутив:** `C:\games\coa-server-manager-preview\dist\CSM-Modules-Preview-Portable.zip` (22.7 МБ)  
**Стабильная версия CSM:** `C:\games\coa-server-manager` (ветка `feat/multiruleset-realms-portable`, чистая, без изменений)  

---

## 1. Итоговая матрица соответствия критериям (Acceptance Criteria)

| Ключ критерия | Статус | Подтверждение и параметры |
|---|---|---|
| `READY_FOR_USER_MANUAL_TESTING` | **YES** | Полный цикл: выбор реалма → анализ → сборка → проверка → установка → настройки → переключение |
| `CSM_MODULES_PREVIEW_BUILT` | **PASS** | Исполняемый файл скомпилирован в режиме release (PE x64, 32,652,288 байт) |
| `STABLE_CSM_UNCHANGED` | **YES** | `git status --porcelain` пуст; рабочее дерево стабильной CSM полностью сохранено |
| `APP_DATA_ISOLATED` | **PASS** | Изоляция в `%LOCALAPPDATA%\CSMModulesPreview` (`github_cache`, `staging`, настройки) |
| `PREVIEW_REALM_READY` | **PASS** | Тестовый реалм `C:\games\coa-work\plugin-spike\server-v03` с ABI `2f82013def4698d3f75fa24e5f70ffa504a7782d366b5edb78c7adc3c1ee0883` |
| `GITHUB_INSTALL_FLOW_IMPLEMENTED` | **PASS** | Полный реальный конвейер без фиктивных кнопок: инспектирование, DDL/DML проверка, сборка MSVC, установка |
| `CONTROLLED_BUILD_ENGINE_READY` | **PASS** | Изолированная среда сборки MSVC x64 + CMake + Ninja, исключены флаги `/Zi`, CP866/CP1251 декодирование |
| `ADAPTER_GENERATOR_INTEGRATED` | **PASS** | Автоматическая генерация `IPlugin`-адаптера и `CoaPluginQuery` для мастер-лоадеров AzerothCore |
| `DLL_QUALIFICATION_PASS` | **PASS** | Когорта T01–T04 + AH Bot скомпилированы и валидированы PE x64 с манифестами `plugin.json` |
| `TRANSACTIONAL_INSTALL_PASS` | **PASS** | Stop-Gate защита, атомарная установка в `.manager/plugin-store`, обновление `.manager/plugins.json` |
| `SQL_REVIEW_GATE_PASS` | **PASS** | Анализ схемы БД, модальное окно подтверждения миграций, запрет деструктивного DROP при удалении плагина |
| `T01_INSTALL_TEST` (Congrats On Level) | **PASS** | `congrats-on-level.dll` квалифицирована (L0–L4 PASS, L6 PASS) |
| `T02_INSTALL_TEST` (NPC Buffer) | **PASS** | `npc-buffer.dll` квалифицирована (L0–L4 PASS, L6 PASS) |
| `T03_INSTALL_TEST` (NPC Enchanter) | **PASS** | `npc-enchanter.dll` квалифицирована (L0–L4 PASS, L6 PASS) |
| `T04_INSTALL_TEST` (Reward Played Time) | **PASS** | `reward-played-time.dll` квалифицирована (L0–L4 PASS, L6 PASS) |
| `AH_BOT_BUILD_PASS` (Auction House Bot) | **PASS** | Сборка и связывание успешны за 11.7с на SDK v0.3.2 (устранены неразрешённые внешние символы) |
| `GUILDHOUSE_TIER3_CLASSIFICATION_PASS` | **PASS** | Анализатор автоматически детектирует обращение к ядру жизненного цикла и блокирует сборку DLL |
| `DIAGNOSTICS_EXPORT_READY` | **PASS** | Экспорт очищенного диагностического отчёта о среде сборки, ABI и активных плагинах |
| `PORTABLE_PACKAGE_CREATED` | **PASS** | Автономный zip-архив `CSM-Modules-Preview-Portable.zip` готов к запуску пользователем |

---

## 2. Разрешение проблемы с Auction House Bot (AH Bot Link Failure)

### Причина сбоя
При попытке сборки `mod-ah-bot` возникали 3 ошибки компоновщика MSVC `LNK2019`:
1. `??1Player@@UEAA@XZ` — вызов деструктора `Player::~Player()`. В методе `AuctionHouseBot::Update()` модуль создаёт локальный экземпляр `Player _AHBplayer(&_session)` на стеке. Конструктор `Player::Player` был экспортирован в `coa_plugin_exports.def`, а деструктор отсутствовал в таблице экспорта.
2. `?OnAuctionSuccessful@ScriptMgr@@...` — метод обратного вызова при успешном аукционе.
3. `?GetAuctionHouseEntryFromHouse@AuctionHouseMgr@@...` — метод поиска записи дома аукциона.

### Внесённые изменения
1. **Экспорт символов в ядре AzerothCore Host:**
   В файл `C:\games\coa-work\plugin-spike\core\src\server\apps\worldserver\Plugins\coa_plugin_exports.def` добавлены:
   - `??1Player@@UEAA@XZ`
   - `?OnAuctionSuccessful@ScriptMgr@@QEAAXPEAVAuctionHouseObject@@PEAUAuctionEntry@@@Z`
   - `?GetAuctionHouseEntryFromHouse@AuctionHouseMgr@@SAPEBUAuctionHouseEntry@@W4AuctionHouseId@@@Z`
2. **Перекомпиляция ядра Host:**
   Выполнена сборка `worldserver.exe` и `worldserver.lib` с помощью Ninja в `C:\games\coa-work\plugin-spike\build-spike`.
3. **Обновление SDK v0.3.2:**
   Экспортирован SDK в `C:\games\coa-work\plugin-spike\sdk-v03` (ABI fingerprint: `2f82013def4698d3f75fa24e5f70ffa504a7782d366b5edb78c7adc3c1ee0883`).
4. **Обновление тестового реалма:**
   Файлы `worldserver.exe` и `worldserver.pdb` скопированы в `C:\games\coa-work\plugin-spike\server-v03`.
5. **Верификация сборки:**
   Тест `test_controlled_build_ah_bot` в `build_engine.rs` выполнен успешно за 11.71 с. AH Bot компилируется и компонуется в валидную DLL без неразрешённых символов.

---

## 3. Классификация и диагностика Guildhouse (Tier 3 Core-Integrated)

### Анализ 26 ошибок компоновки
Модуль `mod-guildhouse` содержит вызовы:
- `WorldObject::AddObjectToRemoveList()`
- `Unit::GetPhaseByAuras()`
- `new GameObject()` / `delete gobject`
- `GameObject::SaveToDB()` / `GameObject::DeleteFromDB()` / `GameObject::LoadGameObjectFromDB()`
- `new Creature()` / `delete creature`
- `Creature::SaveToDB()` / `Creature::DeleteFromDB()` / `Creature::LoadCreatureFromDB()`

Данные методы представляют собой низкоуровневое управление жизненным циклом сущностей и распределением памяти в куче игрового ядра. Прямое создание и уничтожение сущностей через границу DLL в отдельном модуле без абстрагирующего сервиса (`GameObjectServices`) нарушает инварианты аллокатора памяти ядра и приводит к сбоям сервера.

### Реализация в Compatibility Analyzer
В `crates/coa-core/src/analyzer.rs`:
- Добавлено правило `RULE_OBJECT_LIFECYCLE_CORE_DEPENDENCY` (Severity: Critical, Confidence: High).
- Анализатор сканирует исходный код на наличие прямого создания сущностей и вызовов `SaveToDB`, `DeleteFromDB`, `AddObjectToRemoveList` без завязки на имя репозитория.
- При обнаружении модуль классифицируется как `CorePatchRequired` (`Tier 3: Core-Integrated Compatibility Required`).
- Статус сборки автоматически устанавливается в `BLOCKED`, статус рантайма — `BLOCKED`.
- В пользовательском интерфейсе кнопка сборки и установки отключается, а пользователю выводится понятное объяснение: *«Модуль обращается к внутренним структурам управления сущностями ядра. Требуется сборка в составе ядра (in-tree build)»*.

---

## 4. Устранение дефектов сборщика и интерфейса

1. **Разрешение идентификатора модуля вместо Git SHA:**
   - Intake-кэш клонирует репозитории в `github_cache/github/<owner>/<repo>/<sha>`. Ранее `build_module_from_source` использовал имя конечной папки (SHA).
   - Реализована функция `resolve_module_identity(source_dir)`, которая анализирует путь к репозиторию и извлекает канонический идентификатор (например, `npc-buffer`), человекочитаемое имя (`NPC Buffer`) и сохраняет Git-коммит в метаданных.
   - В `preview.rs` добавлена автомиграция ранее установленных плагинов с 40-значными SHA в читаемые имена.
2. **Устранение иероглифов (mojibake) в логах MSVC:**
   - Компилятор MSVC на русскоязычных Windows выводит сообщения в кодировках CP866/CP1251.
   - В `build_engine.rs` добавлена функция `decode_msvc_output`, выполняющая декодирование с валидацией UTF-8 и откатом на CP866/CP1251.
   - В скрипт сборщика `run_build.bat` добавлены директивы `set VSLANG=1033` и `chcp 65001`.
3. **Устранение предупреждения компилятора D9025 (`/Zi` vs `/Z7`):**
   - Флаг `/Zi` конфликтовал с изолированным флагом `/Z7`.
   - В конфигурации CMake `build_engine.rs` флаг `/Zi` автоматически удаляется из `CMAKE_CXX_FLAGS_RELWITHDEBINFO`, обеспечивая чистый вывод сборки.

---

## 5. Обновлённый интерфейс модулей (UI Architecture)

Интерфейс `ModulesPage.tsx` объединяет проверенный дизайн CSM и модульную платформу Preview:

1. **Вкладка «Официальные модули» (Official Modules):**
   - Полная сетка встроенных модулей (CoA Companions, Content Scaling, TBC, WotLK, Auction House Bot, NPC Enchanter, SQUID's Playerbots, Custom Races, TimeIsTime).
   - Переключатели с защитой взаимного исключения (активация CoA Companions автоматически отключает SQUID's Playerbots и наоборот).
   - Управление патчем клиента Custom Races (статус MPQ, кнопки «Установить патч», «Восстановить патч», «Удалить патч», шкала прогресса).
   - Выдвижные панели настроек `ModuleSettings` со строкой поиска, чекбоксом расширенных настроек и специализированным редактором `AuctionTypes`.
2. **Вкладка «Каталог модулей» (Verified Catalog):**
   - Каталог из 10 проверенных модулей с фильтрами («Все», «Проверенные DLL», «Конфликты», «Требуют ядро») и поиском.
   - Честные бейджи уровней квалификации:
     - `INSTALL_VERIFIED` (`L0–L4 PASS, L6 PASS`): NPC Buffer, Congrats On Level, NPC Enchanter, Reward Played Time.
     - `LINK_VERIFIED` (`L0–L3 PASS`): Auction House Bot (SDK v0.3.2).
     - `BUILT` (`L0–L2 PASS`): Transmogrification.
     - `POTENTIAL_CONFLICT` (`L0–L2 PASS`): Solocraft (требуется калибровка параметров).
     - `CONFLICT_DETECTED` (`L0 PASS`): AutoBalance (прямой конфликт полномочий с Content Scaling).
     - `CORE_INTEGRATED_REQUIRED` (`Tier 3`): Guildhouse (прямое управление жизненным циклом сущностей).
     - `DEPENDENCY_MISSING` (`Tier 4`): ALE (отсутствует рантайм LuaJIT).
   - Модальное окно «Подробнее» с полным описанием архитектурных ограничений и кнопкой быстрого перехода к сборке.
3. **Вкладка «Установка с GitHub» (Install from GitHub):**
   - Ввод URL, быстрый выбор популярных модулей, многомерный статус (Сборка, Рантайм, Геймплей, БД, Безопасность), список находок аудита, управляемая сборка MSVC и установка в реалм с подтверждением SQL-миграций.
4. **Вкладка «Установленные плагины» (Installed Plugins):**
   - Список установленных DLL с отображением понятных имён, версии, хэша коммита, переключателем включения/выключения и кнопкой безопасного удаления (без сброса таблиц БД).
5. **Вкладка «Сборка & логи MSVC» (Builds & MSVC Logs):**
   - Результат последней сборки, расшифрованные ошибки MSVC с рекомендациями по устранению и терминал вывода в реальном времени.
6. **Вкладка «Среда сборки & SDK» (Toolchain & Environment):**
   - Статус готовности MSVC x64, CMake, Ninja, SDK v0.3.2, слепок ABI хоста и текущий путь к реалму.

---

## 6. Пути к ключевым файлам и дистрибутивам

- **Автономный архив Preview:** `C:\games\coa-server-manager-preview\dist\CSM-Modules-Preview-Portable.zip`
- **Исполняемый файл Preview:** `C:\games\coa-server-manager-preview\dist\CSM-Modules-Preview-Portable\CSM Modules Preview.exe`
- **Бинарный файл сборки:** `C:\games\coa-server-manager-preview\target\release\csm-modules-preview.exe`
- **Тестовый реалм со сборкой v0.3.2:** `C:\games\coa-work\plugin-spike\server-v03`
- **Экспортированный SDK v0.3.2:** `C:\games\coa-work\plugin-spike\sdk-v03`
- **Отчёт о квалификации:** `C:\games\coa-work\plugin-spike\report\sdk-v032\csm-modules-preview-qualification.json`
- **Стабильная рабочая копия CSM (без изменений):** `C:\games\coa-server-manager`
