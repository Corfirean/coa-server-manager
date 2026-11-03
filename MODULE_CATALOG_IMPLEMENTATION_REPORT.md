# Отчёт об интеграции публичного каталога AzerothCore в Modules V2

**Проект:** CoA Server Manager (CSM)  
**Ветка:** `preview/modules-v2`  
**Рабочая директория:** `C:\games\coa-server-manager-preview`  
**Дата реализации:** Ноябрь 2026 г.  

---

## 1. Резюме выполненных работ

В существующую архитектуру системы **Modules V2** успешно интегрирован официальный публичный каталог модулей AzerothCore (`azerothcore.org/data/catalogue.json`).

Реализация строго следует принципу **непереизобретения колеса**:
- Полностью сохранён существующий конвейер Modules V2:
  - GitHub Intake & Source Cache (`analyzer.rs`)
  - Compatibility Analyzer (`analyze_github_module_for_realm`)
  - Controlled DLL Builder на базе MSVC, CMake, Ninja и `IPlugin` (`build_engine.rs`)
  - Transactional Installer (`preview.rs`)
  - Plugin Registry & Manifest Storage (`plugins.rs`)
  - Server Stop-Gate и SQL Review Gate
- Главное UX-требование выполнено:
  - **Группа A (Recommended for CoA)** всегда отображается на самом верху каталога. Все существующие проверенные модули (включая официальные компоненты ядра и динамические DLL) сохранены в полном объёме с их текущими действиями.
  - **Группа B (AzerothCore Community Modules)** отображается ниже и предоставляет доступ к 500+ модулям сообщества с информацией о звёздах, форках, тегах, статусе совместимости CoA и кнопкой установки, передающей URL репозитория в готовый конвейер `runAnalysis(url)`.
- Блокировка несовместимых модулей (например, `azerothcore/mod-ahbot`) защищена на трёх уровнях: в UI, в анализаторе AST/исходного кода и в сборочном движке (невозможно обойти даже прямой вставкой URL в инпут).

---

## 2. Архитектура и источники данных

### 2.1 Источник каталога
В ходе исследования было установлено, что файл `catalogue.json` в репозитории `azerothcore.github.io` является устаревшим статическим срезом (185 модулей). Реальным актуальным источником каталога AzerothCore является:
- **URL:** `https://www.azerothcore.org/data/catalogue.json`
- **Количество модулей:** 545+ репозиториев
- **Периодичность обновления:** каждые 30 минут через GitHub Actions на Cloudflare Pages CDN
- **Машиночитаемый формат:** JSON-объект с полями `categories` и `repos` (`id`, `name`, `owner`, `html_url`, `stargazers_count`, `forks_count`, `topics`, `category`, `license`, `pushed_at`).

### 2.2 Кэширование, ETag и автономный режим
Реализован движок в `crates/coa-core/src/catalogue.rs`:
1. **HTTP Conditional Requests:** При сетевом запросе отправляется заголовок `If-None-Match: <etag>`. При получении ответа `304 Not Modified` парсинг сетевого тела не производится, обновляется лишь метка времени кэша.
2. **TTL (12 часов):** Если локальный файл кэша свежее 12 часов и пользователь не нажал кнопку принудительного обновления, каталог отдаётся из локального дискового кэша мгновенно без блокирующих сетевых задержек.
3. **Атомарная запись:** Сохранение на диск осуществляется через `fsx::atomic_write_json` в `%LOCALAPPDATA%/coa-manager/catalogue/catalogue_cache.json`.
4. **Offline Fallback:** В случае отсутствия интернета, сбоя DNS или недоступности CDN сервер возвращает последнюю сохранённую копию кэша с флагом `is_offline = true`. В интерфейсе появляется информационная плашка с датой кэша и кнопкой повтора.

### 2.3 Нормализация и дедупликация
Функция `normalize_repo_slug` корректно приводит к единому виду `owner/repo`:
- URL с протоколом: `https://github.com/azerothcore/mod-npc-buffer.git` → `azerothcore/mod-npc-buffer`
- SSH-ссылки: `git@github.com:azerothcore/mod-npc-buffer.git` → `azerothcore/mod-npc-buffer`
- Ссылки на поддиректории: `https://github.com/Corfirean/azerothcore-wotlk-coa/tree/coa-bots/...`

---

## 3. Кураторский оверлей CoA (CoA Modules Overlay)

В `crates/coa-core/src/coa-modules-overlay.json` создан структурированный оверлей совместимости с ядром Conquest of Azeroth.

### 3.1 Статусы совместимости
1. `Recommended` — модуль оптимизирован для CoA и проверен на практике.
2. `Verified` — динамический плагин DLL компилируется и функционирует штатно.
3. `RequiresAdapter` — требует генерации адаптера точки входа `IPlugin`.
4. `Unverified` — сторонний модуль сообщества (по умолчанию).
5. `Incompatible` — содержит известные конфликты с механизмами CoA.
6. `Blocked` — модуль категорически запрещён к установке из-за разрушения специфики CoA (например, конфликтующие боты аукциона или параллельные реализации companion AI).

### 3.2 Защита на уровне движка
В `crates/coa-core/src/analyzer.rs` и `crates/coa-core/src/build_engine.rs` добавлена проверка `catalogue::is_blocked_repo`:
- Анализатор немедленно фиксирует критическое замечание (`CRITICAL`) с причиной блокировки и присваивает статус `SourceIncompatible`.
- Сборочный движок `build_engine::build_module_from_source` прерывает сборку до вызова MSVC/CMake с понятной ошибкой: `Module '...' is blocked for Conquest of Azeroth: ...`.

---

## 4. Tauri IPC и клиентский API

1. **Команда Tauri (`src-tauri/src/lib.rs`):**
   ```rust
   #[tauri::command]
   async fn get_public_module_catalog(
       force_refresh: Option<bool>,
   ) -> std::result::Result<coa_core::catalogue::PublicCatalogueResponse, UiError> {
       let cache_dir = data_dir().join("catalogue");
       let force = force_refresh.unwrap_or(false);
       blocking(move || {
           coa_core::catalogue::fetch_or_load_catalogue(&cache_dir, force)
       })
       .await
   }
   ```
   Команда зарегистрирована в `invoke_handler!`.

2. **Клиентский API (`src/lib/api.ts`):**
   - Добавлены TypeScript-интерфейсы `PublicCatalogueItem`, `PublicCatalogueResponse`, `CoaCompatibilityStatus`.
   - Экспортирован типизированный метод:
     ```typescript
     getPublicModuleCatalog: (forceRefresh = false) =>
       invoke<PublicCatalogueResponse>("get_public_module_catalog", { forceRefresh }),
     ```

---

## 5. Пользовательский интерфейс (`ModulesPage.tsx`)

### 5.1 Иерархия отображения
1. **Группа A — Рекомендовано для CoA:**
   - 13 модулей `VERIFIED_CATALOG` отображаются в самом начале.
   - Имеют золотой бейдж `★ CoA Recommended` / `CoA Verified`.
   - Сохраняют все быстрые действия: для официальных — включение/выключение в ядре, для динамических DLL — прямая сборка и установка.
2. **Группа B — Каталог сообщества AzerothCore:**
   - Все остальные модули каталога azerothcore.org (за вычетом модулей из группы A во избежание дублирования).
   - Информативная карточка: название, автор, описание, звёзды GitHub, форки, категория, лицензия SPDX, теги topics.
   - Бейдж статуса CoA (`CoaStatusBadge`):
     - `Recommended` (золотой)
     - `Verified` (зелёный)
     - `RequiresAdapter` (голубой)
     - `Community AC` (нейтральный)
     - `Incompatible` (янтарный с предупреждением)
     - `Blocked` (красный с причиной блокировки)
3. **Действия с модулями сообщества:**
   - Для уже установленных: индикатор `✓ Установлен` и кнопка `Настроить`.
   - Для заблокированных: заблокированная кнопка `⛔ Заблокирован` с подсказкой причины.
   - Для доступных модулей: кнопка `Собрать и установить`, которая устанавливает `githubUrl`, запускает `runAnalysis(url)` и переводит пользователя на контролируемую сборку `startControlledBuild(url)`.
   - Кнопка `Подробнее`: открывает модальное окно с полным набором метаданных репозитория, лицензией, веткой, историей обновлений и инженерными комментариями.

### 5.2 Поиск, фильтры и сортировка
- **Поиск:** фильтрация по названию, репозиторию, автору, описанию и тегам.
- **Фильтры:**
  - `Все` (сумма групп A и B)
  - `Рекомендовано CoA`
  - `Официальные ядра`
  - `Проверенные DLL`
  - `Сообщество AC`
  - `Несовместимые / Блок`
- **Сортировка модулей сообщества:**
  - `★ По звёздам` (по умолчанию)
  - `🕒 По дате обновления`
  - `🔤 По имени (A-Z)`
- **Пагинация (Load More):** отображение порциями по 24 модуля с кнопкой `Показать ещё (+24)` и счётчиком показанных элементов.
- **Счётчик вкладки:** бейдж на вкладке «Каталог модулей» отображает общее число доступных модулей (`publicCatalog.total_count` ~ 545).

---

## 6. Тестирование и верификация

### 6.1 Модульные тесты бэкенда (`coa-core`)
Успешно пройдены все тесты каталога и смежных подсистем:
- `catalogue::tests::test_slug_normalization` — проверка нормализации URL, SSH-алиасов и суффиксов `.git`
- `catalogue::tests::test_overlay_loading_and_blocked_detection` — корректность загрузки оверлея и выявление блокированных модулей (`azerothcore/mod-ahbot`)
- `catalogue::tests::test_official_json_parsing_and_normalization` — парсинг структуры `catalogue.json` и применение оверлея
- `catalogue::tests::test_disk_caching_and_offline_fallback` — проверка ETag, TTL и возврата из автономного кэша
- `analyzer::tests::*` (17 тестов) — отсутствие регрессий в AST-анализаторе
- `build_engine::tests::test_controlled_build_cohort_t01_t04` и `test_controlled_build_ah_bot` — успешная проверка сборочного движка

### 6.2 Проверка фронтенда
- `tsc --project tsconfig.json --noEmit` — **0 ошибок, полная типизация**.
- `vite build` — **успешная сборка production-бандла** клиента за 4.12 сек.
- `cargo check` — **чистая компиляция Tauri бэкенда** без предупреждений.

---

## 7. Локальные коммиты в ветке `preview/modules-v2`

Все изменения закоммичены локально в рабочей ветке `preview/modules-v2` четырьмя атомарными коммитами:

1. `4e661e6` — `feat(core): implement AzerothCore module catalogue engine, overlay and caching`
   - Создан `crates/coa-core/src/catalogue.rs`
   - Создан `crates/coa-core/src/coa-modules-overlay.json`
   - Зарегистрирован модуль в `crates/coa-core/src/lib.rs`
   - Добавлен план интеграции `MODULE_CATALOG_V2_INTEGRATION_PLAN.md`
2. `6bbdef1` — `feat(analyzer): enforce CoA module blocklist in analyzer and build engine`
   - Добавлена блокировка в `crates/coa-core/src/analyzer.rs`
   - Добавлена проверка безопасности в `crates/coa-core/src/build_engine.rs`
3. `c53fc64` — `feat(ipc): expose public module catalogue Tauri command and API client`
   - Зарегистрирована Tauri команда `get_public_module_catalog` в `src-tauri/src/lib.rs`
   - Экспортированы типы и API-метод в `src/lib/api.ts`
4. `d0c57f9` — `feat(ui): integrate dual-group public AzerothCore catalogue into ModulesPage`
   - Реализованы две группы (Группа A: Рекомендовано для CoA, Группа B: Сообщество AzerothCore)
   - Добавлены бейджи статусов, поиск, фильтры, сортировка, пагинация, модальное окно деталей, плашка офлайн-режима и вызов `runAnalysis`

**Внимание:** В соответствии с инструкцией слияние в `master`, создание PR и push во внешний репозиторий **не выполнялись**. Все изменения готовы к локальному тестированию и ревью.
