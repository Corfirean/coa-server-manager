import { Fragment, useCallback, useEffect, useRef, useState, type ComponentType, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  AlertTriangle,
  Ban,
  Bot,
  CheckCircle2,
  Clock,
  Database,
  Download,
  ExternalLink,
  Flame,
  Gavel,
  GitFork,
  Hammer,
  Layers,
  Loader2,
  MoreHorizontal,
  Plug,
  Puzzle,
  RefreshCw,
  Scaling,
  Search,
  Settings2,
  Shield,
  Snowflake,
  Sparkles,
  Star,
  Swords,
  Terminal,
  Trash2,
  Users,
  X,
  XCircle,
  type LucideProps,
} from "lucide-react";
import {
  api,
  asUiError,
  type BuildResult,
  type CoaCompatibilityStatus,
  type CompatibilityReport,
  type CustomRacesClientStatus,
  type CustomRacesProgress,
  type InstalledPreviewPlugin,
  type ModuleSetting,
  type ModuleView,
  type PublicCatalogueItem,
  type PublicCatalogueResponse,
  type ToolchainStatus,
  type UiError,
} from "@/lib/api";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { useHuman, useI18n, useSchemaText, type Key } from "@/i18n";
import { auctionWeightKey, canonicalModuleValue, moduleEnableKey, moduleField, retiredModuleKeys } from "@/lib/moduleSettings";
import { AuctionTypes } from "@/screens/AuctionTypes";
import { RaceAvailabilitySettings } from "@/screens/RaceAvailabilitySettings";
import type { UnsavedChangesGuard } from "@/lib/unsavedGuard";

/** The GitHub mark */
function GithubMark(props: { className?: string }) {
  return (
    <svg viewBox="0 0 16 16" fill="currentColor" aria-hidden className={props.className}>
      <path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82a7.6 7.6 0 0 1 4 0c1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.01 8.01 0 0 0 16 8c0-4.42-3.58-8-8-8z" />
    </svg>
  );
}

/** Icon and colors of a module's square tile */
const TILES: Record<string, { icon: ComponentType<LucideProps>; from: string; to: string }> = {
  clock: { icon: Clock, from: "from-indigo-400", to: "to-violet-800" },
  bot: { icon: Bot, from: "from-emerald-500", to: "to-teal-700" },
  scaling: { icon: Scaling, from: "from-violet-500", to: "to-indigo-700" },
  plug: { icon: Plug, from: "from-amber-500", to: "to-orange-700" },
  tbc: { icon: Flame, from: "from-lime-500", to: "to-emerald-800" },
  wotlk: { icon: Snowflake, from: "from-sky-400", to: "to-blue-700" },
  gavel: { icon: Gavel, from: "from-yellow-500", to: "to-amber-700" },
  sparkles: { icon: Sparkles, from: "from-fuchsia-500", to: "to-purple-700" },
  swords: { icon: Swords, from: "from-rose-500", to: "to-red-800" },
  races: { icon: Users, from: "from-cyan-500", to: "to-blue-800" },
};
const DEFAULT_TILE = { icon: Puzzle, from: "from-slate-500", to: "to-slate-700" };

const STATUS: Record<string, { label: Key; hint: Key; tone: string }> = {
  soon: { label: "mod.status.soon", hint: "mod.hint.soon", tone: "border-violet-400/50 bg-violet-400/10 text-violet-300" },
  early: { label: "mod.status.early", hint: "mod.hint.early", tone: "border-warn/50 bg-warn/10 text-warn" },
  beta: { label: "mod.status.beta", hint: "mod.hint.beta", tone: "border-sky-400/50 bg-sky-400/10 text-sky-300" },
  release: { label: "mod.status.release", hint: "mod.hint.release", tone: "border-ok/50 bg-ok/10 text-ok" },
};

function SettingsPanel({ className, children }: { className?: string; children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    ref.current?.scrollIntoView({ block: "nearest", behavior: "smooth" });
  }, []);
  return (
    <div ref={ref} className={className}>
      <Card className="border-gold/50 p-5">{children}</Card>
    </div>
  );
}

function StatusBadge({ status }: { status: string }) {
  const { t } = useI18n();
  const s = STATUS[status] ?? STATUS.release;
  return (
    <span className={cn("shrink-0 rounded-full border px-2 py-0.5 text-[11px] font-medium uppercase tracking-wide", s.tone)} title={t(s.hint)}>
      {t(s.label)}
    </span>
  );
}

function Tile({ icon, off }: { icon: string; off: boolean }) {
  const tile = TILES[icon] ?? DEFAULT_TILE;
  const Icon = tile.icon;
  return (
    <div
      aria-hidden
      className={cn(
        "flex h-16 w-16 shrink-0 items-center justify-center rounded-2xl bg-gradient-to-br shadow-lg ring-1 ring-white/10 transition",
        tile.from,
        tile.to,
        off && "opacity-40 grayscale"
      )}
    >
      <Icon className="h-8 w-8 text-white drop-shadow" strokeWidth={1.75} />
    </div>
  );
}

export type TabId = "installed" | "catalog" | "builds" | "toolchain" | "github";

interface VerifiedCatalogModule {
  id: string;
  name: string;
  repo: string;
  category: "official" | "utility" | "economy" | "social" | "profession" | "scaling";
  description: { ru: string; en: string };
  qualification: "OPTIMIZED_COA" | "OFFICIAL_CORE" | "INSTALL_VERIFIED" | "LINK_VERIFIED" | "BUILT";
  tier: string;
  verificationLevel: string;
  notes: { ru: string; en: string };
  sqlRequired: boolean;
  canInstall: boolean;
  isOfficial?: boolean;
  icon?: string;
}

const OFFICIAL_IDS = new Set([
  "companions",
  "content-scaling",
  "tbc-content",
  "wotlk-content",
  "ah-bot",
  "npc-enchanter",
  "playerbots",
  "custom-races",
  "time-is-time",
  "client-compat",
]);

function cleanModuleName(name: string, id: string): string {
  if (name && (name.length !== 40 || !/^[0-9a-f]+$/i.test(name))) {
    return name;
  }
  if (id === "npc-buffer" || id.includes("npc_buffer") || name.includes("npc_buffer")) return "NPC Buffer";
  if (id === "ah-bot" || id.includes("ah_bot") || name.includes("ah_bot")) return "Auction House Bot";
  if (id === "npc-enchanter" || id.includes("enchanter")) return "NPC Enchanter";
  if (id === "congrats-on-level" || id.includes("congrats")) return "Congrats On Level";
  if (id === "reward-played-time" || id.includes("reward_played")) return "Reward Played Time";
  if (id === "solocraft" || id.includes("solocraft")) return "Solocraft";
  if (id && id.length === 40 && /^[0-9a-f]+$/i.test(id)) return `Module (${id.slice(0, 8)})`;
  return id || name;
}

const VERIFIED_CATALOG: VerifiedCatalogModule[] = [
  // Optimized CoA Modules
  {
    id: "companions",
    name: "CoA Companions",
    repo: "https://github.com/Corfirean/mod-coa-playerbots",
    category: "official",
    description: {
      ru: "Боты-компаньоны для групп, подземелий и гильдии.",
      en: "Companion bots for groups, dungeons and your guild.",
    },
    qualification: "OPTIMIZED_COA",
    tier: "Tier 1: Dynamic DLL",
    verificationLevel: "PRODUCTION_SUPPORTED",
    notes: {
      ru: "Оптимизированный модуль ботов-компаньонов для сервера. Взаимоисключение со SQUID's Playerbots.",
      en: "Optimized companion bots module for server. Mutually exclusive with Playerbots.",
    },
    sqlRequired: false,
    canInstall: true,
    isOfficial: true,
    icon: "bot",
  },
  {
    id: "content-scaling",
    name: "Content Scaling",
    repo: "https://github.com/Corfirean/mod-coa-content-scaling",
    category: "official",
    description: {
      ru: "Подстраивает подземелья и рейды под вашу группу, есть соло-режимы.",
      en: "Scales dungeons and raids to your group, with solo modes.",
    },
    qualification: "OPTIMIZED_COA",
    tier: "Tier 1: Dynamic DLL",
    verificationLevel: "PRODUCTION_SUPPORTED",
    notes: {
      ru: "Оптимизированный модуль динамического масштабирования подземелий.",
      en: "Optimized dynamic dungeon scaling module.",
    },
    sqlRequired: false,
    canInstall: true,
    isOfficial: true,
    icon: "scaling",
  },
  {
    id: "tbc-content",
    name: "TBC Content",
    repo: "https://github.com/Corfirean/mod-coa-content-scaling",
    category: "official",
    description: {
      ru: "Подземелья и рейды Запределья, подогнанные под ваши уровни.",
      en: "Outland dungeons and raids, scaled to your level range.",
    },
    qualification: "OPTIMIZED_COA",
    tier: "Content Pack",
    verificationLevel: "PRODUCTION_SUPPORTED",
    notes: {
      ru: "Оптимизированный пакет контента Burning Crusade для Content Scaling.",
      en: "Optimized Burning Crusade content pack for Content Scaling.",
    },
    sqlRequired: false,
    canInstall: true,
    isOfficial: true,
    icon: "tbc",
  },
  {
    id: "wotlk-content",
    name: "WotLK Content",
    repo: "https://github.com/Corfirean/mod-coa-content-scaling",
    category: "official",
    description: {
      ru: "Подземелья и рейды Нордскола, подогнанные под ваши уровни.",
      en: "Northrend dungeons and raids, scaled to your level range.",
    },
    qualification: "OPTIMIZED_COA",
    tier: "Content Pack",
    verificationLevel: "PRODUCTION_SUPPORTED",
    notes: {
      ru: "Оптимизированный пакет контента Wrath of the Lich King для Content Scaling.",
      en: "Optimized Wrath of the Lich King content pack for Content Scaling.",
    },
    sqlRequired: false,
    canInstall: true,
    isOfficial: true,
    icon: "wotlk",
  },
  {
    id: "playerbots",
    name: "SQUID's Playerbots",
    repo: "https://github.com/Corfirean/mod-playerbots",
    category: "official",
    description: {
      ru: "Альтернативная система ботов со случайными ботами и полным ИИ.",
      en: "An alternative bot system with random bots and full AI.",
    },
    qualification: "OPTIMIZED_COA",
    tier: "Tier 1: Dynamic DLL",
    verificationLevel: "PRODUCTION_SUPPORTED",
    notes: {
      ru: "Оптимизированная система случайных ботов SQUID. Взаимоисключение с CoA Companions.",
      en: "Optimized SQUID random bots system. Mutually exclusive with CoA Companions.",
    },
    sqlRequired: true,
    canInstall: true,
    isOfficial: true,
    icon: "swords",
  },
  {
    id: "custom-races",
    name: "Custom Races",
    repo: "https://github.com/Corfirean/azerothcore-wotlk-coa",
    category: "official",
    description: {
      ru: "Пользовательские расы (Esteria и другие) с расширенной внешностью и патчем клиента.",
      en: "Playable custom races (Esteria and additional races) with extended appearances.",
    },
    qualification: "OPTIMIZED_COA",
    tier: "Client Patch",
    verificationLevel: "PRODUCTION_SUPPORTED",
    notes: {
      ru: "Оптимизированный модуль рас CoA. Требуется клиентский MPQ-патч.",
      en: "Optimized CoA custom races module. Requires client MPQ patch.",
    },
    sqlRequired: true,
    canInstall: true,
    isOfficial: true,
    icon: "races",
  },
  {
    id: "time-is-time",
    name: "TimeIsTime",
    repo: "https://github.com/Corfirean/azerothcore-wotlk-coa",
    category: "official",
    description: {
      ru: "Настройка скорости смены дня и ночи и начального времени игрового мира.",
      en: "Adjust the day and night cycle speed and starting game time.",
    },
    qualification: "OPTIMIZED_COA",
    tier: "Core Module",
    verificationLevel: "PRODUCTION_SUPPORTED",
    notes: {
      ru: "Оптимизированный модуль синхронизации игрового времени с сервером.",
      en: "Optimized world time synchronization module.",
    },
    sqlRequired: false,
    canInstall: false,
    isOfficial: true,
    icon: "sparkles",
  },

  // Supported Dynamic DLL Modules (Verified & Compiling)
  {
    id: "npc-buffer",
    name: "NPC Buffer",
    repo: "https://github.com/azerothcore/mod-npc-buffer",
    category: "utility",
    description: {
      ru: "NPC-баффер для наложения положительных эффектов, снятия усталости и лечения персонажей.",
      en: "NPC buff master offering spells, resurrection sickness removal, and player restoration.",
    },
    qualification: "INSTALL_VERIFIED",
    tier: "Tier 1: Dynamic DLL",
    verificationLevel: "L0-L4 PASS, L6 PASS",
    notes: {
      ru: "Сборка, связывание и динамическая загрузка в реалм подтверждены. Требуется импорт SQL.",
      en: "Compilation, linking and dynamic realm load verified. Requires SQL migration.",
    },
    sqlRequired: true,
    canInstall: true,
    icon: "plug",
  },
  {
    id: "ah-bot",
    name: "Auction House Bot",
    repo: "https://github.com/azerothcore/mod-ah-bot",
    category: "official",
    description: {
      ru: "Наполняет аукцион реалистичными настраиваемыми лотами.",
      en: "Fills the auction house with realistic, adjustable listings.",
    },
    qualification: "OPTIMIZED_COA",
    tier: "Tier 1: Dynamic DLL",
    verificationLevel: "PRODUCTION_SUPPORTED",
    notes: {
      ru: "Оптимизированный модуль аукциона виртуальных персонажей для CoA.",
      en: "Optimized virtual characters auction house module for CoA.",
    },
    sqlRequired: false,
    canInstall: true,
    isOfficial: true,
    icon: "gavel",
  },
  {
    id: "congrats-on-level",
    name: "Congrats On Level",
    repo: "https://github.com/azerothcore/mod-congrats-on-level",
    category: "social",
    description: {
      ru: "Анимации, салюты и оповещения в чат при повышении уровня игрока.",
      en: "Server-wide or local celebrations, fireworks and announcements on level up.",
    },
    qualification: "INSTALL_VERIFIED",
    tier: "Tier 1: Dynamic DLL",
    verificationLevel: "L0-L4 PASS, L6 PASS",
    notes: {
      ru: "Полный цикл жизненного цикла динамического плагина проверен на SDK v0.3.",
      en: "Full lifecycle verified against CoA Plugin SDK v0.3.",
    },
    sqlRequired: false,
    canInstall: true,
    icon: "sparkles",
  },
  {
    id: "npc-enchanter",
    name: "NPC Enchanter",
    repo: "https://github.com/azerothcore/mod-npc-enchanter",
    category: "official",
    description: {
      ru: "НПС, который зачаровывает ваше снаряжение.",
      en: "An NPC that enchants your gear for you.",
    },
    qualification: "OPTIMIZED_COA",
    tier: "Tier 1: Dynamic DLL",
    verificationLevel: "PRODUCTION_SUPPORTED",
    notes: {
      ru: "Оптимизированный модуль зачарования экипировки у НПС для CoA.",
      en: "Optimized NPC equipment enchanting module for CoA.",
    },
    sqlRequired: false,
    canInstall: true,
    isOfficial: true,
    icon: "sparkles",
  },
  {
    id: "reward-played-time",
    name: "Reward Played Time",
    repo: "https://github.com/azerothcore/mod-reward-played-time",
    category: "social",
    description: {
      ru: "Периодическая выдача наград и валюты игрокам за время, проведённое на сервере.",
      en: "Periodic rewards and tokens granted to players based on active playtime.",
    },
    qualification: "INSTALL_VERIFIED",
    tier: "Tier 1: Dynamic DLL",
    verificationLevel: "L0-L4 PASS, L6 PASS",
    notes: {
      ru: "Сборка и установка подтверждены. Содержит миграции базы данных characters.",
      en: "Build and install verified. Contains characters DB migrations.",
    },
    sqlRequired: true,
    canInstall: true,
    icon: "plug",
  },
  

];


function CoaStatusBadge({ status, conflictReason }: { status: CoaCompatibilityStatus; conflictReason?: string | null }) {
  const { locale } = useI18n();
  switch (status) {
    case "Recommended":
      return (
        <span className="rounded bg-gold/15 text-gold border border-gold/40 px-2 py-0.5 text-[11px] font-semibold flex items-center gap-1 shadow-sm">
          <Star className="h-3 w-3 fill-gold text-gold" />
          {locale === "ru" ? "Рекомендовано CoA" : "CoA Recommended"}
        </span>
      );
    case "Verified":
      return (
        <span className="rounded bg-ok/15 text-ok border border-ok/40 px-2 py-0.5 text-[11px] font-medium flex items-center gap-1 shadow-sm">
          <CheckCircle2 className="h-3 w-3 text-ok" />
          {locale === "ru" ? "Проверен CoA" : "CoA Verified"}
        </span>
      );
    case "RequiresAdapter":
      return (
        <span className="rounded bg-sky-500/15 text-sky-400 border border-sky-500/40 px-2 py-0.5 text-[11px] font-medium flex items-center gap-1">
          <Sparkles className="h-3 w-3 text-sky-400" />
          {locale === "ru" ? "Требует адаптер" : "Requires Adapter"}
        </span>
      );
    case "Incompatible":
      return (
        <span className="rounded bg-warn/15 text-warn border border-warn/40 px-2 py-0.5 text-[11px] font-medium flex items-center gap-1" title={conflictReason || undefined}>
          <AlertTriangle className="h-3 w-3 text-warn" />
          {locale === "ru" ? "Несовместим" : "Incompatible"}
        </span>
      );
    case "Blocked":
      return (
        <span className="rounded bg-bad/20 text-bad border border-bad/50 px-2 py-0.5 text-[11px] font-semibold flex items-center gap-1" title={conflictReason || undefined}>
          <Ban className="h-3 w-3 text-bad" />
          {locale === "ru" ? "Заблокирован" : "Blocked"}
        </span>
      );
    default:
      return (
        <span className="rounded bg-white/5 text-muted border border-white/10 px-2 py-0.5 text-[11px] font-medium">
          {locale === "ru" ? "Сообщество AC" : "AC Community"}
        </span>
      );
  }
}

/** Categories of the curated CoA list (the public catalogue items get theirs from the backend). */
const CURATED_CATEGORY: Record<string, string> = {
  companions: "bots",
  playerbots: "bots",
  "content-scaling": "pve",
  "tbc-content": "pve",
  "wotlk-content": "pve",
  "custom-races": "classes",
  "time-is-time": "world",
  "npc-buffer": "npc",
  "npc-enchanter": "npc",
  "ah-bot": "economy",
  "congrats-on-level": "progression",
  "reward-played-time": "progression",
};
function curatedCategory(id: string): string {
  return CURATED_CATEGORY[id] ?? "other";
}

function humanizeRawError(raw: string, locale: string): string {
  if (raw.includes("[NO_LOADER_FOUND]")) {
    return locale === "ru"
      ? "В репозитории не найдена C++ точка входа Add*Scripts. Модуль не содержит исходного кода C++ для компиляции в DLL (возможно, это скрипт Lua/Eluna или чистый SQL)."
      : "No Add*Scripts entrypoint found. This repository contains no C++ source code to build into a DLL (it may be a Lua/Eluna script or SQL-only).";
  }
  if (raw.includes("No C++ (.cpp) source files found")) {
    return locale === "ru"
      ? "В репозитории не найдено файлов C++ (.cpp). Модуль не может быть собран в бинарный DLL-плагин (возможно, это скрипт Lua/Eluna или клиентский аддон)."
      : "No C++ source files (.cpp) found in repository. Module cannot be built into a dynamic DLL.";
  }
  if (raw.includes("Repository not found") || raw.includes("[REPOSITORY_NOT_FOUND]")) {
    return locale === "ru"
      ? "Репозиторий GitHub не найден или недоступен. Проверьте правильность URL-адреса."
      : "GitHub repository not found or unreachable. Check the repository URL.";
  }
  if (raw.includes("Failed to locate cached repository sources")) {
    return locale === "ru"
      ? "Не удалось найти кэшированные исходные коды. Попробуйте нажать 'Анализировать' снова."
      : "Failed to locate cached repository sources. Try analyzing the module again.";
  }
  if (raw.includes("[REPOSITORY_TOO_LARGE]")) {
    return locale === "ru"
      ? `Размер файла или репозитория превышает допустимый лимит: ${raw.replace(/^\[REPOSITORY_TOO_LARGE\]\s*/, "")}`
      : `Repository or file exceeds size limit: ${raw.replace(/^\[REPOSITORY_TOO_LARGE\]\s*/, "")}`;
  }
  if (raw.includes("[MODULE_BLOCKED]")) {
    return locale === "ru"
      ? `Модуль заблокирован: ${raw}`
      : `Module blocked: ${raw}`;
  }
  if (raw.includes("[MANUAL_ADAPTER_REQUIRED]")) {
    return locale === "ru"
      ? `Требуется ручной адаптер точки входа: ${raw}`
      : `Manual adapter required: ${raw}`;
  }
  if (raw.includes("[TOOLCHAIN_ERROR]")) {
    return locale === "ru"
      ? `Ошибка среды сборки: ${raw}`
      : `Toolchain error: ${raw}`;
  }
  return raw;
}

function formatErrorMessage(e: unknown, locale = "ru"): string {
  if (!e) return locale === "ru" ? "Неизвестная ошибка" : "Unknown error";
  if (typeof e === "string" && e !== "[object Object]") {
    return humanizeRawError(e, locale);
  }
  if (e instanceof Error) return humanizeRawError(e.message, locale);
  if (typeof e === "object") {
    const obj = e as {
      technical?: unknown;
      human?: { title?: unknown; message?: unknown };
      message?: unknown;
      error?: unknown;
      reason?: unknown;
    };
    const raw = (
      (typeof obj.technical === "string" && obj.technical !== "[object Object]" && obj.technical) ||
      (obj.human && typeof obj.human.message === "string" && obj.human.message !== "[object Object]" && obj.human.message) ||
      (typeof obj.message === "string" && obj.message !== "[object Object]" && obj.message) ||
      (typeof obj.error === "string" && obj.error !== "[object Object]" && obj.error) ||
      (typeof obj.reason === "string" && obj.reason !== "[object Object]" && obj.reason) ||
      ""
    );
    if (raw) {
      return humanizeRawError(raw, locale);
    }
    try {
      const json = JSON.stringify(e);
      if (json && json !== "{}" && json !== "[]") return json;
    } catch {}
  }
  return locale === "ru" ? "Неизвестная ошибка при сборке модуля" : "Unknown error during module build";
}

function formatDate(dateStr?: string | null): string {
  if (!dateStr) return "—";
  try {
    const d = new Date(dateStr);
    return d.toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
  } catch {
    return dateStr;
  }
}

export function ModulesPage({
  serverId,
  onChanged,
  onRegisterGuard,
}: {
  serverId: string;
  onChanged?: () => void;
  onRegisterGuard?: (guard: UnsavedChangesGuard | null) => void;
}) {
  const { t, locale } = useI18n();
  const human = useHuman();

  // Navigation tab
  const [activeTab, setActiveTab] = useState<TabId>("installed");

  // Official modules state
  const [modules, setModules] = useState<ModuleView[] | null>(null);
  const [openSettings, setOpenSettings] = useState<string | null>(null);
  const toggleLock = useRef(false);
  const [toggling, setToggling] = useState<string | null>(null);
  const [changed, setChanged] = useState(false);
  const [racesPatchStatus, setRacesPatchStatus] = useState<CustomRacesClientStatus | null>(null);
  const [racesInstalling, setRacesInstalling] = useState(false);
  const [racesProgress, setRacesProgress] = useState<CustomRacesProgress | null>(null);
  const [guard, setGuard] = useState<UnsavedChangesGuard | null>(null);

  // Installed dynamic plugins
  const [installedPlugins, setInstalledPlugins] = useState<InstalledPreviewPlugin[]>([]);
  const [error, setError] = useState<UiError | null>(null);
  const [notification, setNotification] = useState<string | null>(null);

  // Catalog search & filter
  const [catalogQuery, setCatalogQuery] = useState("");
  // Four groups that do not overlap: checked by CoA (curated list + community modules CoA recommends or verified),
  // the rest of the AzerothCore community, and modules known not to work with CoA.
  const [catalogFilter, setCatalogFilter] = useState<"all" | "verified" | "community" | "incompatible">("all");
  const [catalogCategory, setCatalogCategory] = useState<string>("all");
  const [allCategoryChips, setAllCategoryChips] = useState(false);
  const [selectedCatalogModule, setSelectedCatalogModule] = useState<VerifiedCatalogModule | null>(null);

  // Public AzerothCore Catalog state
  const [publicCatalog, setPublicCatalog] = useState<PublicCatalogueResponse | null>(null);
  const [publicLoading, setPublicLoading] = useState(false);
  const [publicError, setPublicError] = useState<string | null>(null);
  const [selectedPublicModule, setSelectedPublicModule] = useState<PublicCatalogueItem | null>(null);
  const [catalogSort, setCatalogSort] = useState<"stars" | "name" | "updated">("stars");
  const [communityDisplayLimit, setCommunityDisplayLimit] = useState<number>(24);

  // GitHub Intake & Analysis state
  const [githubUrl, setGithubUrl] = useState("");
  // "Add from a link" stays folded away until asked for; the menu holds the rarely used build tools.
  const [showUrlBar, setShowUrlBar] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const [analyzing, setAnalyzing] = useState(false);
  const [analysisError, setAnalysisError] = useState<string | null>(null);
  const [report, setReport] = useState<CompatibilityReport | null>(null);

  // Toolchain state
  const [toolchain, setToolchain] = useState<ToolchainStatus | null>(null);
  const [msvcInstalling, setMsvcInstalling] = useState(false);
  const [msvcResult, setMsvcResult] = useState<{ ready: boolean; restart_required: boolean; message: string } | null>(null);

  // Controlled Build state
  const [building, setBuilding] = useState(false);
  const [buildResult, setBuildResult] = useState<BuildResult | null>(null);
  const [buildLogs, setBuildLogs] = useState<string[]>([]);
  const [buildProgress, setBuildProgress] = useState<{
    step: string;
    progress_pct: number;
    message: string;
    elapsedSeconds: number;
  } | null>(null);

  // Installation & SQL confirmation modal
  const [installing, setInstalling] = useState(false);
  const [showSqlDialog, setShowSqlDialog] = useState(false);

  // Action busy indicators
  const [actionInProgress, setActionInProgress] = useState<string | null>(null);

  const [racesGuard, setRacesGuard] = useState<UnsavedChangesGuard | null>(null);

  useEffect(() => {
    const guards = [guard, racesGuard].filter((item): item is UnsavedChangesGuard => item !== null);
    onRegisterGuard?.(guards.length ? {
      isDirty: () => guards.some(item => item.isDirty()),
      save: async () => { for (const item of guards) if (item.isDirty() && !await item.save()) return false; return true; },
      discard: () => guards.forEach(item => item.discard()),
    } : null);
    return () => onRegisterGuard?.(null);
  }, [guard, racesGuard, onRegisterGuard]);

  const loadPublicCatalog = useCallback(async (forceRefresh = false) => {
    setPublicLoading(true);
    setPublicError(null);
    try {
      const res = await api.getPublicModuleCatalog(forceRefresh);
      setPublicCatalog(res);
    } catch (e) {
      console.error("Failed to load public module catalog:", e);
      setPublicError(e instanceof Error ? e.message : String(e));
    } finally {
      setPublicLoading(false);
    }
  }, []);

  useEffect(() => {
    if (activeTab === "catalog" && !publicCatalog && !publicLoading) {
      void loadPublicCatalog(false);
    }
  }, [activeTab, publicCatalog, publicLoading, loadPublicCatalog]);

  const loadToolchain = useCallback(async () => {
    try {
      const tc = await api.previewToolchainStatus(serverId);
      setToolchain(tc);
    } catch (e) {
      console.error("Failed to load toolchain status", e);
    }
  }, [serverId]);

  const loadInstalledPlugins = useCallback(async () => {
    try {
      const plugins = await api.previewListInstalledPlugins(serverId);
      setInstalledPlugins(plugins);
    } catch {
      // not critical if realm has no plugins yet
    }
  }, [serverId]);

  const loadRacesStatus = useCallback(async () => {
    try {
      const st = await api.customRacesStatus(serverId);
      setRacesPatchStatus(st);
    } catch {
      // ignore
    }
  }, [serverId]);

  const load = useCallback(async () => {
    try {
      const mods = await api.modulesList(serverId);
      setModules(mods.filter((m) => !m.hidden));
      await loadRacesStatus();
      await loadInstalledPlugins();
      await loadToolchain();
    } catch (e) {
      setError(asUiError(e));
    }
  }, [serverId, loadRacesStatus, loadInstalledPlugins, loadToolchain]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void listen<CustomRacesProgress>("custom-races-progress", (e) => {
      setRacesProgress(e.payload);
    }).then((un) => {
      unlisten = un;
    });
    return () => {
      unlisten?.();
    };
  }, []);

  async function installClientPatch() {
    setRacesInstalling(true);
    setError(null);
    try {
      await api.customRacesInstall(serverId);
      await loadRacesStatus();
      onChanged?.();
    } catch (e) {
      setError(asUiError(e));
    } finally {
      setRacesInstalling(false);
      setRacesProgress(null);
    }
  }

  async function uninstallClientPatch() {
    setRacesInstalling(true);
    setError(null);
    try {
      await api.customRacesUninstall(serverId);
      await loadRacesStatus();
      onChanged?.();
    } catch (e) {
      setError(asUiError(e));
    } finally {
      setRacesInstalling(false);
    }
  }

  async function toggleOfficialModule(m: ModuleView) {
    if (toggleLock.current || racesInstalling) return;
    toggleLock.current = true;
    setToggling(m.id);
    setError(null);
    try {
      // Mutual exclusion check between companions and playerbots (squid)
      if (m.id === "companions" && !m.enabled) {
        const otherBot = modules?.find((x) => (x.id === "playerbots" || x.id === "squid") && x.enabled);
        if (otherBot) {
          await api.moduleSetEnabled(serverId, otherBot.id, false);
        }
      } else if ((m.id === "playerbots" || m.id === "squid") && !m.enabled) {
        const compMod = modules?.find((x) => x.id === "companions" && x.enabled);
        if (compMod) {
          await api.moduleSetEnabled(serverId, "companions", false);
        }
      }
      await api.moduleSetEnabled(serverId, m.id, !m.enabled);
      setChanged(true);
      await load();
      if (m.id === "custom-races") {
        await loadRacesStatus();
      }
      onChanged?.();
    } catch (e) {
      setError(asUiError(e));
      await load();
    } finally {
      toggleLock.current = false;
      setToggling(null);
    }
  }

  async function exportDiagnostics() {
    setActionInProgress("export-diag");
    try {
      const p = await api.previewExportDiagnostics(serverId);
      setNotification(
        locale === "ru" ? `Диагностический отчёт сохранён: ${p}` : `Diagnostic report saved: ${p}`
      );
    } catch (e) {
      setError(asUiError(e));
    } finally {
      setActionInProgress(null);
    }
  }

  async function runAnalysis(urlToAnalyze?: string) {
    const targetUrl = (urlToAnalyze || githubUrl).trim();
    if (!targetUrl) return;
    setGithubUrl(targetUrl);
    setAnalyzing(true);
    setAnalysisError(null);
    setReport(null);
    setBuildResult(null);
    try {
      const rep = await api.analyzeGithubModule(targetUrl, serverId);
      setReport(rep);
    } catch (e: unknown) {
      setAnalysisError(formatErrorMessage(e, locale));
    } finally {
      setAnalyzing(false);
    }
  }

  async function startControlledBuild(urlToBuildParam?: string) {
    const target = (urlToBuildParam || githubUrl || report?.repository_path || "").trim();
    if (!target) return;
    setBuilding(true);
    setActiveTab("builds");
    setBuildLogs(["[CSM Controlled Build Engine] Initializing build environment..."]);
    setBuildProgress({
      step: "PREPARING",
      progress_pct: 10,
      message: locale === "ru" ? "Инициализация среды сборки MSVC и SDK..." : "Initializing MSVC and SDK build environment...",
      elapsedSeconds: 0,
    });

    const startTime = Date.now();
    const timer = setInterval(() => {
      const elapsed = (Date.now() - startTime) / 1000;
      setBuildProgress((prev) => (prev ? { ...prev, elapsedSeconds: elapsed } : null));
    }, 100);

    let unlisten: (() => void) | null = null;
    try {
      try {
        unlisten = await listen<{ step: string; progress_pct: number; message: string; log_line?: string }>(
          "preview_build_progress",
          (event) => {
            const p = event.payload;
            setBuildProgress((prev) => ({
              step: p.step,
              progress_pct: p.progress_pct,
              message: p.message,
              elapsedSeconds: prev ? prev.elapsedSeconds : (Date.now() - startTime) / 1000,
            }));
            if (p.log_line) {
              setBuildLogs((logs) => [...logs, p.log_line!]);
            }
          }
        );
      } catch {}

      setBuildLogs((prev) => [
        ...prev,
        `Target module: ${target}`,
        "Compiling C++ sources against CoA Plugin SDK v0.3 via MSVC...",
      ]);
      const res = await api.previewBuildModule(serverId, target);
      setBuildResult(res);
      setBuildLogs(res.log_output);
      if (res.success) {
        setBuildProgress({
          step: "INSTALLING",
          progress_pct: 95,
          message: locale === "ru" ? "Установка скомпилированного модуля..." : "Installing compiled module into realm...",
          elapsedSeconds: (Date.now() - startTime) / 1000,
        });

        // Automatically install if module doesn't require schema changes
        let autoInstalled = false;
        try {
          await api.previewInstallModule(
            serverId,
            res.module_id,
            res.version,
            false
          );
          setNotification(
            locale === "ru"
              ? `Модуль '${res.display_name || res.module_id}' успешно собран и установлен!`
              : `Module '${res.display_name || res.module_id}' successfully built and installed!`
          );
          await load();
          setActiveTab("installed");
          onChanged?.();
          autoInstalled = true;
        } catch {
          // If auto-install requires manual SQL confirmation, keep build results visible
        }

        if (!autoInstalled) {
          setNotification(
            locale === "ru"
              ? `Сборка успешно завершена! DLL '${res.display_name || res.module_id}' квалифицирована: ${res.dll_sha256?.substring(0, 16)}...`
              : `Build succeeded! DLL '${res.display_name || res.module_id}' qualified: ${res.dll_sha256?.substring(0, 16)}...`
          );
        }
      } else {
        setAnalysisError(
          locale === "ru"
            ? "Сборка завершилась ошибкой. Проверьте диагностику компилятора во вкладке 'Сборка & логи MSVC'."
            : "Build failed. Check compiler diagnostics in the 'Builds & MSVC Logs' tab."
        );
      }
    } catch (e: unknown) {
      const errText = formatErrorMessage(e, locale);
      setAnalysisError(errText);
      setBuildLogs((prev) => [...prev, `[FATAL BUILD ERROR]: ${errText}`]);
    } finally {
      clearInterval(timer);
      unlisten?.();
      setBuilding(false);
    }
  }

  async function handleBuildAndInstall(repoUrl: string) {
    const url = repoUrl.trim();
    if (!url) return;
    setGithubUrl(url);
    setActiveTab("builds");
    setBuilding(true);
    setAnalysisError(null);
    setBuildResult(null);
    setBuildLogs([
      `[CSM Controlled Build Engine] Initializing build pipeline...`,
      `Target repository: ${url}`,
      `Cloning and inspecting C++ sources...`,
    ]);
    setBuildProgress({
      step: "ANALYZING",
      progress_pct: 15,
      message: locale === "ru" ? "Интроспекция репозитория..." : "Inspecting module repository...",
      elapsedSeconds: 0,
    });
    try {
      const rep = await api.analyzeGithubModule(url, serverId);
      setReport(rep);
      await startControlledBuild(rep.repository_path);
    } catch (e: unknown) {
      const errText = formatErrorMessage(e, locale);
      setAnalysisError(errText);
      setBuildLogs((prev) => [...prev, `[FATAL ERROR]: ${errText}`]);
      setBuilding(false);
    }
  }

  function handleInstallClick() {
    if (!buildResult || !buildResult.success) return;
    if (report && report.sql_files.length > 0) {
      setShowSqlDialog(true);
    } else {
      void performInstall(false);
    }
  }

  async function performInstall(applySql: boolean) {
    if (!buildResult) return;
    setShowSqlDialog(false);
    setInstalling(true);
    setError(null);
    try {
      const res = await api.previewInstallModule(
        serverId,
        buildResult.module_id,
        buildResult.version,
        applySql
      );
      setNotification(res.message);
      await load();
      setActiveTab("installed");
      onChanged?.();
    } catch (e) {
      setError(asUiError(e));
    } finally {
      setInstalling(false);
    }
  }

  async function removePlugin(moduleId: string) {
    if (
      !confirm(
        locale === "ru"
          ? `Удалить плагин '${moduleId}'? Базы данных и пользовательские настройки будут сохранены.`
          : `Remove plugin '${moduleId}'? Databases and configurations will be preserved.`
      )
    ) {
      return;
    }
    setActionInProgress(`remove-${moduleId}`);
    setError(null);
    try {
      const msg = await api.previewRemoveModule(serverId, moduleId);
      setNotification(msg);
      await loadInstalledPlugins();
      await load();
      onChanged?.();
    } catch (e) {
      setError(asUiError(e));
    } finally {
      setActionInProgress(null);
    }
  }

  function exportReportJson() {
    if (!report) return;
    const blob = new Blob([JSON.stringify(report, null, 2)], { type: "application/json" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `${report.repository_name}-compatibility-report.json`;
    a.click();
    URL.revokeObjectURL(url);
  }

  const isScalingInstalled = Boolean(
    modules?.some((m) => (m.id === "content-scaling" || m.id === "coa-content-scaling") && m.installed) ||
    installedPlugins?.some((p) => p.id === "content-scaling" || p.id === "coa-content-scaling")
  );

  const installedModules = (modules?.filter((m) => m.installed) ?? []).filter((m) => {
    if (m.id === "tbc-content" || m.id === "wotlk-content") {
      return isScalingInstalled;
    }
    // Dungeon Clear is part of SQUID's Playerbots (switched with it); its settings open from the SQUID card.
    return m.id !== "dungeon-clear";
  });
  const dungeonClear = modules?.find((m) => m.id === "dungeon-clear" && m.installed && m.has_settings) ?? null;
  const selectedModule = modules?.find((m) => m.id === openSettings) ?? null;
  const selectedModuleIndex = installedModules.findIndex((m) => m.id === (openSettings === "dungeon-clear" ? "playerbots" : openSettings));

  // Normalize repo URL to "owner/repo" slug
  function normalizeUrlSlug(url: string): string {
    const clean = url.trim().toLowerCase().replace(/^https?:\/\/github\.com\//, "").replace(/\.git$/, "");
    const parts = clean.split("/tree/")[0].split("/");
    return parts.length >= 2 ? `${parts[parts.length - 2]}/${parts[parts.length - 1]}` : clean;
  }

  // Repositories incompatible with CoA forks (squid playerbots and integrated dungeon clear)
  const EXCLUDED_COMMUNITY_SLUGS = new Set([
    "mod-playerbots/mod-playerbots",
    "azerothcore/mod-playerbots",
    "jrad7/mod-dungeon-clear",
    "nathanhandley/mod-ah-bot-plus",
    "azerothcore/mod-ahbot",
  ]);

  // Set of slugs from VERIFIED_CATALOG to avoid duplicate cards in community list
  const verifiedRepoSlugs = new Set(
    VERIFIED_CATALOG.map((m) => normalizeUrlSlug(m.repo))
  );

  // Group A: Recommended / Verified for CoA
  const searchedVerifiedCatalog = VERIFIED_CATALOG.filter((item) => {
    if (!catalogQuery.trim()) return true;
    const q = catalogQuery.toLowerCase();
    return (
      item.name.toLowerCase().includes(q) ||
      item.id.toLowerCase().includes(q) ||
      item.description.ru.toLowerCase().includes(q) ||
      item.description.en.toLowerCase().includes(q)
    );
  });

  // Group B: AzerothCore Community Modules
  const searchedCommunityCatalog = (publicCatalog?.items ?? [])
    .filter((item) => {
      // Exclude items already curated in Group A, or upstream forks replaced by CoA
      const slug = (item.full_name || item.name || "").toLowerCase();
      const urlSlug = normalizeUrlSlug(item.html_url);
      if (
        verifiedRepoSlugs.has(slug) ||
        verifiedRepoSlugs.has(urlSlug) ||
        EXCLUDED_COMMUNITY_SLUGS.has(slug) ||
        EXCLUDED_COMMUNITY_SLUGS.has(urlSlug) ||
        item.name.toLowerCase() === "mod-playerbots" ||
        item.name.toLowerCase() === "mod-dungeon-clear"
      ) {
        return false;
      }

      if (!catalogQuery.trim()) return true;
      const q = catalogQuery.toLowerCase();
      return (
        (item.name || "").toLowerCase().includes(q) ||
        (item.full_name || "").toLowerCase().includes(q) ||
        (item.owner || "").toLowerCase().includes(q) ||
        (item.description || "").toLowerCase().includes(q) ||
        (item.topics || []).some((t) => (t || "").toLowerCase().includes(q))
      );
    })
    .sort((a, b) => {
      if (catalogSort === "stars") {
        return (b.stars || 0) - (a.stars || 0);
      }
      if (catalogSort === "updated") {
        return (b.pushed_at || "").localeCompare(a.pushed_at || "");
      }
      return (a.name || "").localeCompare(b.name || "");
    });

  // "Checked by CoA" is the Manager's own curated list only; a community module is "incompatible" when the overlay
  // says so and "community" otherwise.
  type CatalogGroup = "verified" | "community" | "incompatible";
  const communityGroup = (status: string | undefined): CatalogGroup =>
    status === "Incompatible" || status === "Blocked" ? "incompatible" : "community";
  const inCategory = (code: string) => catalogCategory === "all" || code === catalogCategory;
  const inGroup = (group: CatalogGroup) => catalogFilter === "all" || group === catalogFilter;
  // Each control counts what it would show with the other one as it is now.
  const catalogCounts: Record<"all" | CatalogGroup, number> = {
    verified: searchedVerifiedCatalog.filter((i) => inCategory(curatedCategory(i.id))).length,
    community: searchedCommunityCatalog.filter((i) => communityGroup(i.coa_status) === "community" && inCategory(i.category ?? "other")).length,
    incompatible: searchedCommunityCatalog.filter((i) => communityGroup(i.coa_status) === "incompatible" && inCategory(i.category ?? "other")).length,
    all: 0,
  };
  catalogCounts.all = catalogCounts.verified + catalogCounts.community + catalogCounts.incompatible;
  const categoryCounts = new Map<string, number>();
  for (const i of inGroup("verified") ? searchedVerifiedCatalog : []) {
    const c = curatedCategory(i.id);
    categoryCounts.set(c, (categoryCounts.get(c) ?? 0) + 1);
  }
  for (const i of searchedCommunityCatalog) {
    if (!inGroup(communityGroup(i.coa_status))) continue;
    const c = i.category ?? "other";
    categoryCounts.set(c, (categoryCounts.get(c) ?? 0) + 1);
  }
  const catalogCategories = (publicCatalog?.categories ?? []).filter((c) => (categoryCounts.get(c.code) ?? 0) > 0 || c.code === catalogCategory);
  const filteredVerifiedCatalog = inGroup("verified") ? searchedVerifiedCatalog.filter((i) => inCategory(curatedCategory(i.id))) : [];
  const filteredCommunityCatalog = searchedCommunityCatalog.filter(
    (i) => catalogFilter !== "verified" && inGroup(communityGroup(i.coa_status)) && inCategory(i.category ?? "other")
  );

  // Helper to check whether any module/plugin is installed
  function checkModuleInstalled(repoOrId: string, name: string) {
    const slug = normalizeUrlSlug(repoOrId);
    const shortName = slug.split("/").pop() || slug;

    const plug = installedPlugins.find((p) => {
      const pId = p.id.toLowerCase();
      const pName = p.name.toLowerCase();
      return pId === slug || pId === shortName || pName === name.toLowerCase() || pName === shortName;
    });
    if (plug) {
      return { installed: true, enabled: plug.enabled, version: plug.version };
    }

    const mod = modules?.find((m) => {
      const mId = m.id.toLowerCase();
      const mName = m.name.toLowerCase();
      return mId === slug || mId === shortName || mName === name.toLowerCase() || mName === shortName;
    });
    if (mod && mod.installed) {
      return { installed: true, enabled: mod.enabled };
    }

    return { installed: false, enabled: false };
  }

  return (
    <div className="flex h-full flex-col p-6 space-y-4 max-w-6xl">
      {/* Header: title, add-from-link, and a menu with the build tools */}
      <div className="flex items-center justify-between gap-4">
        <h1 className="text-2xl font-semibold">{locale === "ru" ? "Модули" : "Modules"}</h1>
        <div className="relative flex items-center gap-2">
          <Button size="sm" variant={showUrlBar ? "primary" : "ghost"} onClick={() => setShowUrlBar((v) => !v)} aria-expanded={showUrlBar} className="text-xs">
            <GithubMark className="mr-1.5 h-3.5 w-3.5" />
            {locale === "ru" ? "Добавить по ссылке" : "Add from a link"}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            aria-label={locale === "ru" ? "Ещё" : "More"}
            aria-haspopup="menu"
            aria-expanded={menuOpen}
            onClick={() => setMenuOpen((v) => !v)}
            className="h-8 w-8 p-0"
          >
            <MoreHorizontal className="h-4 w-4" />
          </Button>
          {menuOpen && (
            <>
              <div className="fixed inset-0 z-10" onClick={() => setMenuOpen(false)} aria-hidden />
              <div role="menu" className="absolute right-0 top-9 z-20 w-56 rounded-md border border-line bg-card p-1 text-sm shadow-lg">
                {([
                  ["builds", Terminal, locale === "ru" ? "Сборки и логи" : "Builds and logs"],
                  ["toolchain", Shield, locale === "ru" ? "Среда сборки и SDK" : "Build tools and SDK"],
                ] as const).map(([tab, Icon, label]) => (
                  <button
                    key={tab}
                    role="menuitem"
                    onClick={() => {
                      setActiveTab(tab);
                      setMenuOpen(false);
                    }}
                    className="flex w-full items-center gap-2 rounded px-2.5 py-1.5 text-left text-muted hover:bg-white/5 hover:text-ink"
                  >
                    <Icon className="h-4 w-4" /> {label}
                    {tab === "builds" && building && <Loader2 className="ml-auto h-3.5 w-3.5 animate-spin text-gold" />}
                  </button>
                ))}
                <button
                  role="menuitem"
                  disabled={actionInProgress === "export-diag"}
                  onClick={() => {
                    setMenuOpen(false);
                    void exportDiagnostics();
                  }}
                  className="flex w-full items-center gap-2 rounded px-2.5 py-1.5 text-left text-muted hover:bg-white/5 hover:text-ink"
                >
                  <Download className="h-4 w-4" /> {locale === "ru" ? "Экспорт отчёта" : "Export diagnostics"}
                </button>
              </div>
            </>
          )}
        </div>
      </div>

      {/* Notification Banner */}
      {notification && (
        <div className="rounded border border-ok/30 bg-ok/10 p-3 text-xs text-ok flex items-center justify-between">
          <span>✓ {notification}</span>
          <button onClick={() => setNotification(null)} className="text-muted hover:text-ink">
            <X className="h-4 w-4" />
          </button>
        </div>
      )}

      {/* Error Banner */}
      {error && (
        <div className="rounded border border-bad/30 bg-bad/10 p-3 text-xs text-bad flex items-center justify-between">
          <span>⚠ {error.technical || (error.human ? human(error.human).message : "Unknown error")}</span>
          <button onClick={() => setError(null)} className="text-muted hover:text-ink">
            <X className="h-4 w-4" />
          </button>
        </div>
      )}

      {/* Build tools: said only when something is missing */}
      {toolchain && activeTab !== "toolchain" && (() => {
        const missing = [
          !toolchain.msvc_available && "MSVC",
          !toolchain.cmake_available && "CMake",
          !toolchain.ninja_available && "Ninja",
          !toolchain.sdk_available && "Plugin SDK",
        ].filter(Boolean) as string[];
        return missing.length > 0 ? (
          <div className="flex flex-wrap items-center justify-between gap-2 rounded border border-warn/30 bg-warn/5 px-3 py-2 text-xs" role="status">
            <span className="flex items-center gap-2 text-warn">
              <AlertTriangle className="h-4 w-4" />
              {locale === "ru" ? "Для сборки модулей не хватает: " : "Missing for building modules: "}
              {missing.join(", ")}
            </span>
            <Button size="sm" variant="ghost" className="h-7 text-xs" onClick={() => setActiveTab("toolchain")}>
              {locale === "ru" ? "Подробнее" : "Details"}
            </Button>
          </div>
        ) : null;
      })()}

      {/* Add from a GitHub link: folded away until asked for, or while a build from it is in progress */}
      {(showUrlBar || report || analysisError || building) && (
      <Card className="border-line bg-card/60 p-3.5 shadow-sm">
        <div className="flex flex-col sm:flex-row items-stretch sm:items-center gap-2">
          <div className="relative flex-1">
            <GithubMark className="absolute left-3 top-2.5 h-4 w-4 text-muted" />
            <input
              type="text"
              value={githubUrl}
              onChange={(e) => setGithubUrl(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && githubUrl.trim()) {
                  void runAnalysis();
                }
              }}
              placeholder={
                locale === "ru"
                  ? "Укажите ссылку на GitHub-репозиторий модуля (например, https://github.com/azerothcore/mod-npc-buffer)..."
                  : "Enter GitHub repository URL (e.g., https://github.com/azerothcore/mod-npc-buffer)..."
              }
              className="w-full rounded border border-line bg-bg pl-9 pr-3 py-1.5 text-xs text-ink placeholder:text-muted/60 focus:border-gold focus:outline-none"
            />
          </div>

          <div className="flex items-center gap-2">
            <Button
              size="sm"
              variant="ghost"
              onClick={() => void runAnalysis()}
              disabled={analyzing || !githubUrl.trim()}
              className="text-xs h-8 px-3"
            >
              {analyzing ? <Loader2 className="h-3.5 w-3.5 animate-spin mr-1.5" /> : <Search className="h-3.5 w-3.5 mr-1.5" />}
              {locale === "ru" ? "Анализировать" : "Analyze"}
            </Button>

            <Button
              size="sm"
              variant="primary"
              onClick={() => void handleBuildAndInstall(githubUrl)}
              disabled={building || analyzing || !githubUrl.trim()}
              className="text-xs h-8 px-3.5 font-medium"
            >
              {building ? <Loader2 className="h-3.5 w-3.5 animate-spin mr-1.5" /> : <Hammer className="h-3.5 w-3.5 mr-1.5" />}
              {locale === "ru" ? "Собрать и установить" : "Build & Install"}
            </Button>
          </div>
        </div>

        {/* Compact analysis banner when report is ready */}
        {report && (
          <div className="mt-3 border-t border-line/60 pt-3 flex flex-wrap items-center justify-between gap-2 text-xs">
            <div className="flex items-center gap-2">
              <span className="font-semibold text-ink">{report.repository_name}</span>
              <span className="font-mono text-muted text-[11px]">@{report.commit.substring(0, 8)}</span>
              <span className="rounded bg-ok/10 text-ok border border-ok/30 px-2 py-0.5 text-[10px] uppercase font-mono">
                {report.classification}
              </span>
              {report.sql_files.length > 0 && (
                <span className="rounded bg-black/40 text-muted px-1.5 py-0.5 font-mono text-[10px]">
                  {report.sql_files.length} SQL
                </span>
              )}
            </div>

            <div className="flex items-center gap-2">
              <Button size="sm" variant="ghost" onClick={exportReportJson} className="text-xs h-7 px-2">
                {locale === "ru" ? "Экспорт JSON" : "Export JSON"}
              </Button>
              <Button size="sm" variant="primary" onClick={() => void startControlledBuild()} disabled={building} className="text-xs h-7 px-2.5">
                {locale === "ru" ? "Перейти к сборке" : "Build"}
              </Button>
              {buildResult?.success && (
                <Button size="sm" variant="primary" onClick={handleInstallClick} disabled={installing} className="bg-ok hover:bg-ok/90 text-xs h-7 px-2.5">
                  {locale === "ru" ? "Установить в реалм" : "Install"}
                </Button>
              )}
              <button onClick={() => setReport(null)} className="text-muted hover:text-ink ml-1" title="Скрыть">
                <X className="h-4 w-4" />
              </button>
            </div>
          </div>
        )}

        {analysisError && (
          <div className="mt-2 text-xs text-bad flex items-center justify-between">
            <span>⚠ {analysisError}</span>
            <button onClick={() => setAnalysisError(null)} className="text-muted hover:text-ink">
              <X className="h-3.5 w-3.5" />
            </button>
          </div>
        )}

        {/* Global Live Build Progress Bar */}
        {building && buildProgress && (
          <div className="mt-3 border-t border-line/60 pt-3 space-y-2">
            <div className="flex items-center justify-between text-xs">
              <span className="flex items-center gap-2 font-medium text-ink">
                <Loader2 className="h-3.5 w-3.5 animate-spin text-gold" />
                <span>{buildProgress.message || buildProgress.step}</span>
              </span>
              <span className="font-mono font-semibold text-gold">
                {buildProgress.progress_pct}% ({buildProgress.elapsedSeconds.toFixed(1)}s)
              </span>
            </div>
            <div className="h-2 w-full overflow-hidden rounded-full bg-black/60 border border-line/40">
              <div
                className="h-full bg-gradient-to-r from-amber-500 via-yellow-400 to-amber-300 transition-all duration-300 ease-out"
                style={{ width: `${Math.max(5, Math.min(100, buildProgress.progress_pct))}%` }}
              />
            </div>
          </div>
        )}
      </Card>
      )}

      {/* Tabs, styled like the settings page; the build tools open from the menu with a way back */}
      <div className="flex flex-wrap items-end gap-1 border-b border-line" role="tablist">
        {([
          ["installed", locale === "ru" ? "Установленные" : "Installed", installedModules.length],
          ["catalog", locale === "ru" ? "Каталог" : "Catalog", publicCatalog ? catalogCounts.all : null],
        ] as const).map(([tab, label, count]) => (
          <button
            key={tab}
            role="tab"
            aria-selected={activeTab === tab}
            onClick={() => setActiveTab(tab)}
            className={cn(
              "-mb-px cursor-pointer border-b-2 px-3 py-2 text-sm",
              activeTab === tab ? "border-gold text-ink" : "border-transparent text-muted hover:text-ink"
            )}
          >
            {label}
            {count !== null && count > 0 && <span className="ml-1.5 text-xs text-muted tabular-nums">{count}</span>}
          </button>
        ))}
        {(activeTab === "builds" || activeTab === "toolchain") && (
          <span className="-mb-px border-b-2 border-gold px-3 py-2 text-sm text-ink">
            {activeTab === "builds" ? (locale === "ru" ? "Сборки и логи" : "Builds and logs") : (locale === "ru" ? "Среда сборки и SDK" : "Build tools and SDK")}
          </span>
        )}
      </div>

      {/* Sticky Active Build Banner when user is outside the builds tab */}
      {building && buildProgress && activeTab !== "builds" && (
        <div
          onClick={() => setActiveTab("builds")}
          className="cursor-pointer rounded-lg border border-gold/40 bg-gold/10 p-3 flex items-center justify-between shadow transition-colors hover:bg-gold/15 animate-in fade-in"
        >
          <div className="flex items-center gap-3">
            <Loader2 className="h-4 w-4 animate-spin text-gold" />
            <div>
              <div className="flex items-center gap-2">
                <span className="text-xs font-semibold uppercase tracking-wider text-gold">
                  {locale === "ru" ? "Идет сборка модуля" : "Building module"}
                </span>
                <span className="text-xs text-zinc-300 font-mono">[{buildProgress.step}]</span>
                <span className="text-xs text-muted">— {buildProgress.message}</span>
              </div>
            </div>
          </div>
          <div className="flex items-center gap-3">
            <div className="w-32 h-1.5 overflow-hidden rounded-full bg-black/60 border border-line/40">
              <div
                className="h-full bg-gradient-to-r from-amber-500 to-yellow-400 transition-all duration-300"
                style={{ width: `${Math.max(5, Math.min(100, buildProgress.progress_pct))}%` }}
              />
            </div>
            <span className="font-mono text-xs font-bold text-gold">{buildProgress.progress_pct}%</span>
            <span className="text-[11px] font-mono text-zinc-400">({buildProgress.elapsedSeconds.toFixed(1)}s)</span>
          </div>
        </div>
      )}

      {/* TAB 0: INSTALLED MODULES */}
      {activeTab === "installed" && (
        <div className="space-y-4">
          <div className="flex items-center justify-between">
            <div>
              <p className="text-xs text-muted">
                {locale === "ru"
                  ? "Установленные модули и динамические плагины сервера. Включайте, выключайте и настраивайте параметры каждого модуля."
                  : "Installed server modules and dynamic DLL plugins. Enable, disable, and configure parameters for each module."}
              </p>
            </div>
            {changed && (
              <span className="rounded bg-gold/10 border border-gold/30 px-2.5 py-1 text-xs text-gold">
                {t("mod.restart")}
              </span>
            )}
          </div>

          <div className="grid gap-4 sm:grid-cols-2">
            {modules === null && !error && (
              <div className="flex items-center gap-2 text-muted text-sm sm:col-span-2">
                <Loader2 className="h-4 w-4 animate-spin" />
                {t("overview.checking")}
              </div>
            )}
            {modules !== null && installedModules.length === 0 && (
              <Card className="p-8 text-center text-muted sm:col-span-2 space-y-2">
                <p className="text-sm font-medium text-ink">
                  {locale === "ru" ? "Нет установленных модулей" : "No installed modules"}
                </p>
                <p className="text-xs text-muted">
                  {locale === "ru"
                    ? "Выберите модули из Каталога или укажите ссылку на GitHub выше для установки."
                    : "Choose modules from the Catalog or provide a GitHub URL above to install."}
                </p>
                <Button
                  size="sm"
                  variant="primary"
                  onClick={() => setActiveTab("catalog")}
                  className="mt-2 text-xs"
                >
                  <Layers className="h-3.5 w-3.5 mr-1.5" />
                  {locale === "ru" ? "Открыть каталог модулей" : "Open Modules Catalog"}
                </Button>
              </Card>
            )}
            {installedModules.map((m, i) => {
              const isDynamic = m.component_type === "DYNAMIC_DLL" || (!OFFICIAL_IDS.has(m.id) && (installedPlugins.some((p) => p.id === m.id) || m.id.length === 40));
              const canRemove = m.can_remove ?? isDynamic;
              return (
                <Fragment key={m.id}>
                  <Card
                    className={cn(
                      "flex flex-col gap-4 p-5 transition-colors",
                      (!m.installed || m.status === "soon") && "opacity-60",
                      openSettings === m.id && "border-gold/50"
                    )}
                  >
                    <div className="flex items-start gap-4">
                      <Tile icon={m.icon} off={!m.enabled || !m.installed || m.status === "soon"} />
                      <div className="min-w-0 flex-1 pr-2">
                        <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
                          <h3 className="font-semibold leading-tight text-ink truncate max-w-[220px] sm:max-w-[280px]" title={cleanModuleName(m.name, m.id)}>{cleanModuleName(m.name, m.id)}</h3>
                          {m.version && (
                            <span className="font-mono text-xs font-normal text-muted">{m.version}</span>
                          )}
                          <StatusBadge status={m.status} />
                          {m.component_type === "DYNAMIC_DLL" && (
                            <span className="rounded bg-sky-500/10 text-sky-400 border border-sky-500/30 px-1.5 py-0.2 text-[10px] font-mono uppercase">
                              DLL
                            </span>
                          )}
                          {m.component_type === "CORE_EMBEDDED" && (
                            <span className="rounded bg-amber-500/10 text-amber-400 border border-amber-500/30 px-1.5 py-0.2 text-[10px] font-mono uppercase">
                              EMBEDDED
                            </span>
                          )}
                          {m.component_type === "CONTENT_PACK" && (
                            <span className="rounded bg-purple-500/10 text-purple-400 border border-purple-500/30 px-1.5 py-0.2 text-[10px] font-mono uppercase">
                              CONTENT
                            </span>
                          )}
                          {m.component_type === "CLIENT_PATCH" && (
                            <span className="rounded bg-emerald-500/10 text-emerald-400 border border-emerald-500/30 px-1.5 py-0.2 text-[10px] font-mono uppercase">
                              CLIENT PATCH
                            </span>
                          )}
                        </div>
                        <p className="mt-1.5 line-clamp-2 text-xs text-muted">
                          {m.description[locale] ?? m.description.en}
                        </p>
                        {m.id === "companions" && (
                          <p className="mt-1 text-[11px] text-zinc-400">
                            {locale === "ru"
                              ? "Взаимоисключение: не работает одновременно со SQUID's Playerbots."
                              : "Mutual exclusion: cannot run concurrently with SQUID's Playerbots."}
                          </p>
                        )}
                        {(m.id === "playerbots" || m.id === "squid") && (
                          <p className="mt-1 text-[11px] text-zinc-400">
                            {locale === "ru"
                              ? "Взаимоисключение: отключает CoA Companions при активации."
                              : "Mutual exclusion: disables CoA Companions upon activation."}
                          </p>
                        )}
                        {m.id === "playerbots" && dungeonClear && (
                          <p className="mt-1 text-[11px] text-zinc-400">
                            {locale === "ru"
                              ? "Включает Dungeon Clear — тактику подземелий для ботов; включается и выключается вместе со SQUID."
                              : "Includes Dungeon Clear, dungeon tactics for the bots; it is switched on and off with SQUID."}
                          </p>
                        )}
                      </div>
                      {m.installed && m.switchable && (
                        <button
                          role="switch"
                          aria-checked={m.enabled}
                          aria-label={t("mod.switch", { name: m.name })}
                          aria-busy={toggling === m.id}
                          disabled={toggling !== null || (m.id === "custom-races" && racesInstalling)}
                          onClick={() => void toggleOfficialModule(m)}
                          className={cn(
                            "relative mt-1 h-6 w-11 shrink-0 cursor-pointer rounded-full transition-colors",
                            m.enabled ? "bg-gold" : "bg-white/15",
                            m.id === "custom-races" && racesInstalling && "opacity-50 cursor-not-allowed"
                          )}
                        >
                          <span
                            className={cn(
                              "absolute top-0.5 h-5 w-5 rounded-full bg-white transition-all",
                              m.enabled ? "left-[22px]" : "left-0.5"
                            )}
                          />
                        </button>
                      )}
                      {m.installed && !m.switchable && m.status !== "soon" && (
                        <span className="mt-1.5 shrink-0 text-xs text-muted">{t("mod.alwaysOn")}</span>
                      )}
                    </div>

                    {toggling === m.id && (
                      <p role="status" className="flex items-center gap-2 text-xs text-gold">
                        <Loader2 className="h-3.5 w-3.5 animate-spin" aria-hidden />
                        {t("mod.switching")}
                      </p>
                    )}

                    {/* Custom Races Client Patch status badge */}
                    {m.id === "custom-races" && racesPatchStatus && (
                      <div className="text-[11px] text-muted flex items-center gap-1.5">
                        <span className={cn(
                          "h-2 w-2 rounded-full",
                          racesPatchStatus.state === "enabled" ? "bg-ok" : racesPatchStatus.state === "disabled" ? "bg-muted" : "bg-warn"
                        )} />
                        <span>
                          {locale === "ru" ? "Патч клиента: " : "Client patch: "}
                          <strong>
                            {racesPatchStatus.state === "enabled"
                              ? (locale === "ru" ? "Активен" : "Active")
                              : racesPatchStatus.state === "disabled"
                              ? (locale === "ru" ? "Отключен" : "Disabled")
                              : (locale === "ru" ? "Не установлен" : "Not installed")}
                          </strong>
                        </span>
                      </div>
                    )}

                    <div className="mt-auto flex items-center justify-between border-t border-line/40 pt-3">
                      <div className="flex items-center gap-2">
                        <button
                          onClick={() => void api.openLink(m.repo)}
                          title={t("mod.github")}
                          aria-label={`${t("mod.github")}: ${m.name}`}
                          className="flex h-7 w-7 cursor-pointer items-center justify-center rounded-md border border-line text-muted transition-colors hover:border-gold/50 hover:text-ink"
                        >
                          <GithubMark className="h-3.5 w-3.5" />
                        </button>
                        {!m.page && m.installed && (m.has_settings || m.id === "custom-races") && m.status !== "soon" && (
                          <Button
                            size="sm"
                            variant="ghost"
                            aria-expanded={openSettings === m.id}
                            onClick={() => setOpenSettings(openSettings === m.id ? null : m.id)}
                            className={cn("text-xs h-7 px-2.5", openSettings === m.id && "bg-gold/15 text-gold border border-gold/40")}
                          >
                            <Settings2 className="h-3.5 w-3.5 mr-1" aria-hidden /> {t("mod.settings")}
                          </Button>
                        )}
                        {m.id === "playerbots" && m.installed && dungeonClear && (
                          <Button
                            size="sm"
                            variant="ghost"
                            aria-expanded={openSettings === "dungeon-clear"}
                            onClick={() => setOpenSettings(openSettings === "dungeon-clear" ? null : "dungeon-clear")}
                            className={cn("text-xs h-7 px-2.5", openSettings === "dungeon-clear" && "bg-gold/15 text-gold border border-gold/40")}
                          >
                            <Settings2 className="h-3.5 w-3.5 mr-1" aria-hidden /> Dungeon Clear
                          </Button>
                        )}
                      </div>

                      {canRemove && (
                        <Button
                          size="sm"
                          variant="ghost"
                          onClick={() => removePlugin(m.id)}
                          disabled={actionInProgress === `remove-${m.id}`}
                          className="text-bad hover:text-bad hover:bg-bad/10 text-xs h-7 px-2"
                        >
                          <Trash2 className="h-3.5 w-3.5 mr-1" />
                          {locale === "ru" ? "Удалить" : "Remove"}
                        </Button>
                      )}
                    </div>
                  </Card>

                  {/* Inline settings drawer */}
                  {selectedModule?.has_settings &&
                    !selectedModule.page &&
                    selectedModuleIndex >= 0 &&
                    Math.floor(selectedModuleIndex / 2) === Math.floor(i / 2) &&
                    (i % 2 === 1 || i === installedModules.length - 1) && (
                      <SettingsPanel key={selectedModule.id} className="sm:col-span-2">
                        <div className="flex items-center justify-between border-b border-line pb-3 mb-3">
                          <h3 className="font-semibold text-ink text-sm flex items-center gap-2">
                            <Settings2 className="h-4 w-4 text-gold" />
                            {t("mod.settingsOf", { name: selectedModule.name })}
                          </h3>
                          <button
                            onClick={() => setOpenSettings(null)}
                            className="text-muted hover:text-ink text-xs flex items-center gap-1"
                          >
                            <X className="h-4 w-4" />
                          </button>
                        </div>
                        <ModuleSettings
                          serverId={serverId}
                          module={selectedModule}
                          onSaved={() => setChanged(true)}
                          onRegisterGuard={setGuard}
                        />
                        {selectedModule.id === "custom-races" && (
                          <div className="space-y-4 border-t border-line/40 pt-4 mt-4">
                            <div className="flex items-center justify-between">
                              <h4 className="text-xs font-semibold uppercase tracking-wider text-ink flex items-center gap-2">
                                <Download className="h-4 w-4 text-gold" />
                                {locale === "ru" ? "Управление патчем клиента MPQ" : "Client MPQ Patch Management"}
                              </h4>
                              <span className="text-xs text-muted">
                                {locale === "ru" ? "Версия: " : "Version: "}
                                {racesPatchStatus?.version ?? (locale === "ru" ? "не записана" : "not recorded")}
                              </span>
                            </div>

                            {racesInstalling && racesProgress ? (
                              <div className="rounded-lg border border-gold/30 bg-gold/5 p-3">
                                <div className="flex items-center justify-between gap-3 text-xs">
                                  <span className="flex min-w-0 items-center gap-2 font-medium text-ink">
                                    <Loader2 className="h-3.5 w-3.5 shrink-0 animate-spin text-gold" />
                                    <span className="truncate">{racesProgress.step}</span>
                                  </span>
                                  <span className="shrink-0 font-mono text-xs font-semibold text-gold">
                                    {Math.round(racesProgress.percent * 100)}%
                                  </span>
                                </div>
                                <div
                                  className="mt-2.5 h-2 w-full overflow-hidden rounded-full bg-white/10"
                                  role="progressbar"
                                  aria-valuenow={Math.round(racesProgress.percent * 100)}
                                  aria-valuemin={0}
                                  aria-valuemax={100}
                                >
                                  <div
                                    className="h-full rounded-full bg-gold transition-all duration-300"
                                    style={{ width: `${Math.round(racesProgress.percent * 100)}%` }}
                                  />
                                </div>
                              </div>
                            ) : (
                              <div className="flex flex-wrap items-center justify-between gap-2 border border-line/40 rounded-lg p-3 bg-black/20 text-xs">
                                <div className="flex items-center gap-2">
                                  {racesPatchStatus?.state === "enabled" && (
                                    <span className="inline-flex items-center gap-1.5 rounded-full bg-ok/10 px-2.5 py-1 text-xs text-ok border border-ok/30">
                                      <span className="h-1.5 w-1.5 rounded-full bg-ok" />
                                      {locale === "ru" ? "Патч клиента: Включен" : "Client Patch: Active"}
                                    </span>
                                  )}
                                  {racesPatchStatus?.state === "disabled" && (
                                    <span
                                      className="inline-flex items-center gap-1.5 rounded-full bg-white/5 px-2.5 py-1 text-xs text-muted border border-white/15"
                                      title="MPQ files disabled with .disabled suffix"
                                    >
                                      <span className="h-1.5 w-1.5 rounded-full bg-muted" />
                                      {locale === "ru"
                                        ? "Патч клиента: Отключен (.disabled)"
                                        : "Client Patch: Disabled (.disabled)"}
                                    </span>
                                  )}
                                  {racesPatchStatus?.state === "not-installed" && (
                                    <span className="inline-flex items-center gap-1.5 rounded-full bg-warn/10 px-2.5 py-1 text-xs text-warn border border-warn/30">
                                      <span className="h-1.5 w-1.5 rounded-full bg-warn" />
                                      {locale === "ru" ? "Патч клиента: Не установлен" : "Client Patch: Not Installed"}
                                    </span>
                                  )}
                                  {racesPatchStatus?.state === "partially-installed" && (
                                    <span className="inline-flex items-center gap-1.5 rounded-full bg-warn/10 px-2.5 py-1 text-xs text-warn border border-warn/30">
                                      <span className="h-1.5 w-1.5 rounded-full bg-warn" />
                                      {locale === "ru"
                                        ? `Патч клиента: ${racesPatchStatus.installedMpqs}/${racesPatchStatus.totalMpqs} MPQ`
                                        : `Client Patch: ${racesPatchStatus.installedMpqs}/${racesPatchStatus.totalMpqs} MPQ`}
                                    </span>
                                  )}
                                </div>
                                <div className="flex items-center gap-2">
                                  <Button
                                    size="sm"
                                    variant="secondary"
                                    className="h-7 px-3 text-xs border border-gold/40 hover:border-gold"
                                    disabled={toggling !== null || racesInstalling || !racesPatchStatus?.clientPath}
                                    onClick={() => void installClientPatch()}
                                  >
                                    {racesInstalling && <Loader2 className="mr-1.5 h-3.5 w-3.5 animate-spin" />}
                                    {racesPatchStatus?.state === "not-installed"
                                      ? locale === "ru"
                                        ? "Установить патч клиента"
                                        : "Install Client Patch"
                                      : locale === "ru"
                                      ? "Восстановить патч клиента"
                                      : "Repair Client Patch"}
                                  </Button>
                                  {racesPatchStatus && racesPatchStatus.state !== "not-installed" && (
                                    <Button
                                      size="sm"
                                      variant="ghost"
                                      disabled={toggling !== null || racesInstalling}
                                      onClick={() => void uninstallClientPatch()}
                                      className="text-xs"
                                    >
                                      {locale === "ru" ? "Удалить патч клиента" : "Uninstall Client Patch"}
                                    </Button>
                                  )}
                                </div>
                              </div>
                            )}

                            <RaceAvailabilitySettings
                              serverId={serverId}
                              blocked={racesInstalling || toggling !== null}
                              onSaved={() => setChanged(true)}
                              onRegisterGuard={setRacesGuard}
                            />
                          </div>
                        )}
                      </SettingsPanel>
                    )}
                </Fragment>
              );
            })}
          </div>
        </div>
      )}

      {/* TAB 1: AZEROTHCORE & COA MODULE CATALOG */}
      {activeTab === "catalog" && (
        <div className="space-y-5">
          {/* Offline notice banner */}
          {publicCatalog?.is_offline && (
            <div className="flex items-center justify-between rounded border border-warn/30 bg-warn/5 px-3 py-2 text-xs text-warn">
              <span className="flex items-center gap-2">
                <AlertTriangle className="h-4 w-4 shrink-0" />
                {locale === "ru"
                  ? `Нет связи: каталог из кэша от ${formatDate(publicCatalog.cached_at)}`
                  : `Offline: catalogue from the cache of ${formatDate(publicCatalog.cached_at)}`}
              </span>
              <Button size="sm" variant="ghost" onClick={() => void loadPublicCatalog(true)} disabled={publicLoading} className="h-6 text-xs">
                {locale === "ru" ? "Повторить" : "Retry"}
              </Button>
            </div>
          )}

          {publicError && (
            <div className="flex items-center justify-between rounded border border-bad/40 bg-bad/10 px-3 py-2 text-xs text-bad">
              <span>{publicError}</span>
              <Button size="sm" variant="ghost" onClick={() => void loadPublicCatalog(true)} disabled={publicLoading} className="h-6 text-xs">
                {locale === "ru" ? "Повторить" : "Retry"}
              </Button>
            </div>
          )}

          {/* Search, compatibility, sort, refresh */}
          <div className="flex flex-wrap items-center gap-2">
            <div className="relative min-w-[220px] flex-1">
              <Search className="pointer-events-none absolute left-3 top-2.5 h-3.5 w-3.5 text-muted" />
              <input
                type="text"
                value={catalogQuery}
                onChange={(e) => setCatalogQuery(e.target.value)}
                placeholder={locale === "ru" ? "Поиск по названию, автору, тегу" : "Search by name, author, topic"}
                aria-label={locale === "ru" ? "Поиск по каталогу" : "Search the catalogue"}
                className="selectable w-full rounded-md border border-line bg-bg py-2 pl-8 pr-3 text-sm text-ink placeholder:text-muted/60 focus:border-gold focus:outline-none"
              />
            </div>
            <select
              value={catalogFilter}
              onChange={(e) => setCatalogFilter(e.target.value as typeof catalogFilter)}
              aria-label={locale === "ru" ? "Совместимость" : "Compatibility"}
              className="rounded-md border border-line bg-bg px-2.5 py-2 text-xs text-ink focus:border-gold focus:outline-none"
            >
              <option value="all">{locale === "ru" ? `Совместимость: все (${catalogCounts.all})` : `Compatibility: all (${catalogCounts.all})`}</option>
              <option value="verified">{locale === "ru" ? `Проверено CoA (${catalogCounts.verified})` : `Checked by CoA (${catalogCounts.verified})`}</option>
              <option value="community">{locale === "ru" ? `Сообщество (${catalogCounts.community})` : `Community (${catalogCounts.community})`}</option>
              <option value="incompatible">{locale === "ru" ? `Несовместимые (${catalogCounts.incompatible})` : `Incompatible (${catalogCounts.incompatible})`}</option>
            </select>
            <select
              value={catalogSort}
              onChange={(e) => setCatalogSort(e.target.value as "stars" | "name" | "updated")}
              aria-label={locale === "ru" ? "Сортировка" : "Sort"}
              className="rounded-md border border-line bg-bg px-2.5 py-2 text-xs text-ink focus:border-gold focus:outline-none"
            >
              <option value="stars">{locale === "ru" ? "по звёздам" : "by stars"}</option>
              <option value="updated">{locale === "ru" ? "по обновлению" : "recently updated"}</option>
              <option value="name">{locale === "ru" ? "по имени" : "by name"}</option>
            </select>
            <Button
              size="sm"
              variant="ghost"
              onClick={() => void loadPublicCatalog(true)}
              disabled={publicLoading}
              aria-label={locale === "ru" ? "Обновить каталог" : "Refresh the catalogue"}
              title={locale === "ru" ? "Обновить каталог" : "Refresh the catalogue"}
              className="h-8 w-8 p-0"
            >
              <RefreshCw className={cn("h-3.5 w-3.5", publicLoading && "animate-spin text-gold")} />
            </Button>
          </div>

          {/* Categories */}
          {catalogCategories.length > 0 && (
            <div className="flex flex-wrap items-center gap-1.5 text-xs" role="radiogroup" aria-label={locale === "ru" ? "Категория" : "Category"}>
              {[{ code: "all", ru: "Все", en: "All" }, ...catalogCategories]
                .filter((c, index) => allCategoryChips || index <= 8 || c.code === catalogCategory)
                .map((c) => {
                  const on = catalogCategory === c.code;
                  const count = c.code === "all" ? catalogCounts.all : (categoryCounts.get(c.code) ?? 0);
                  return (
                    <button
                      key={c.code}
                      role="radio"
                      aria-checked={on}
                      onClick={() => setCatalogCategory(c.code)}
                      className={cn(
                        "rounded-full border px-2.5 py-1 transition-colors",
                        on ? "border-gold/50 bg-gold/15 text-gold" : "border-line text-muted hover:text-ink"
                      )}
                    >
                      {locale === "ru" ? c.ru : c.en}
                      {c.code !== "all" && <span className="ml-1 tabular-nums opacity-70">{count}</span>}
                    </button>
                  );
                })}
              {catalogCategories.length > 8 && (
                <button onClick={() => setAllCategoryChips((v) => !v)} className="px-1.5 py-1 text-muted underline-offset-2 hover:text-ink hover:underline">
                  {allCategoryChips ? (locale === "ru" ? "свернуть" : "fewer") : (locale === "ru" ? `ещё ${catalogCategories.length - 8}` : `${catalogCategories.length - 8} more`)}
                </button>
              )}
            </div>
          )}

          {publicLoading && !publicCatalog && (
            <div className="flex items-center gap-2 p-4 text-xs text-muted">
              <Loader2 className="h-4 w-4 animate-spin text-gold" />
              {locale === "ru" ? "Загрузка каталога AzerothCore…" : "Loading the AzerothCore catalogue…"}
            </div>
          )}

          {/* The list: CoA's own modules first, then the AzerothCore catalogue */}
          <div className="divide-y divide-line rounded-lg border border-line">
            {filteredVerifiedCatalog.length + filteredCommunityCatalog.length === 0 && !publicLoading && (
              <p className="p-6 text-center text-xs text-muted">{locale === "ru" ? "Ничего не найдено." : "Nothing found."}</p>
            )}

            {filteredVerifiedCatalog.map((item) => {
              const matchedModule = modules?.find(
                (m) => m.id === item.id || m.id === `coa-${item.id}` || m.id === `mod-coa-${item.id}` || m.name.toLowerCase() === item.name.toLowerCase()
              );
              const matchedPlugin = installedPlugins?.find(
                (p) => p.id === item.id || p.id === `coa-${item.id}` || p.name.toLowerCase() === item.name.toLowerCase()
              );
              const isInstalled = Boolean(matchedModule?.installed || matchedPlugin);
              const isEnabled = Boolean(matchedModule?.enabled ?? matchedPlugin?.enabled ?? false);
              const enable = async (id: string) => {
                try {
                  await api.moduleSetEnabled(serverId, id, true);
                  setChanged(true);
                  await load();
                  setOpenSettings(id);
                  setActiveTab("installed");
                  onChanged?.();
                } catch (e) {
                  setError(asUiError(e));
                }
              };
              const cat = (publicCatalog?.categories ?? []).find((c) => c.code === curatedCategory(item.id));
              return (
                <div key={item.id} className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-3 px-3 py-2.5 sm:grid-cols-[minmax(0,1fr)_150px_60px_auto]">
                  <button onClick={() => setSelectedCatalogModule(item)} className="min-w-0 text-left">
                    <span className="flex items-center gap-2">
                      <span className="truncate text-sm font-medium text-ink">{item.name}</span>
                      <span className="shrink-0 rounded border border-gold/40 bg-gold/10 px-1.5 text-[10px] font-semibold text-gold">CoA</span>
                    </span>
                    <span className="block truncate text-xs text-muted">{locale === "ru" ? item.description.ru : item.description.en}</span>
                  </button>
                  <span className="hidden truncate text-xs text-muted sm:block">{cat ? (locale === "ru" ? cat.ru : cat.en) : ""}</span>
                  <span className="hidden text-xs text-muted sm:block" />
                  <div className="flex justify-end">
                    {isInstalled ? (
                      <Button
                        size="sm"
                        variant="ghost"
                        onClick={() => {
                          if (matchedModule) setOpenSettings(matchedModule.id);
                          setActiveTab("installed");
                        }}
                        className="h-7 px-2.5 text-xs text-ok"
                      >
                        <CheckCircle2 className="mr-1 h-3.5 w-3.5" />
                        {locale === "ru" ? (isEnabled ? "Включён" : "Установлен") : isEnabled ? "Active" : "Installed"}
                      </Button>
                    ) : (item.id === "tbc-content" || item.id === "wotlk-content") && !isScalingInstalled ? (
                      <span className="px-2 text-xs text-muted">{locale === "ru" ? "Нужен Content Scaling" : "Needs Content Scaling"}</span>
                    ) : item.id === "tbc-content" || item.id === "wotlk-content" || item.id === "time-is-time" ? (
                      <Button size="sm" variant="primary" onClick={() => void enable(item.id)} className="h-7 px-3 text-xs">
                        {locale === "ru" ? "Включить" : "Enable"}
                      </Button>
                    ) : item.id === "custom-races" ? (
                      <Button
                        size="sm"
                        variant="primary"
                        onClick={() => {
                          setActiveTab("installed");
                          setOpenSettings("custom-races");
                        }}
                        className="h-7 px-3 text-xs"
                      >
                        {locale === "ru" ? "Установить патч" : "Install patch"}
                      </Button>
                    ) : (
                      <Button size="sm" variant="primary" onClick={() => void handleBuildAndInstall(item.repo)} disabled={building} className="h-7 px-3 text-xs">
                        {locale === "ru" ? "Собрать" : "Build"}
                      </Button>
                    )}
                  </div>
                </div>
              );
            })}

            {filteredCommunityCatalog.slice(0, communityDisplayLimit).map((item) => {
              const status = checkModuleInstalled(item.html_url, item.name);
              const isBlocked = item.coa_status === "Blocked";
              const isIncompatible = item.coa_status === "Incompatible";
              const cat = (publicCatalog?.categories ?? []).find((c) => c.code === (item.category ?? "other"));
              return (
                <div key={item.id} className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-3 px-3 py-2.5 sm:grid-cols-[minmax(0,1fr)_150px_60px_auto]">
                  <button onClick={() => setSelectedPublicModule(item)} className="min-w-0 text-left">
                    <span className="flex items-center gap-2">
                      <span className="truncate text-sm font-medium text-ink">{item.name}</span>
                      <span className="shrink-0 truncate font-mono text-[11px] text-muted">{item.owner}</span>
                      {(isBlocked || isIncompatible) && (
                        <span
                          className={cn("shrink-0 rounded px-1.5 text-[10px]", isBlocked ? "bg-bad/15 text-bad" : "bg-warn/15 text-warn")}
                          title={item.conflict_reason ?? undefined}
                        >
                          {isBlocked ? (locale === "ru" ? "заблокирован" : "blocked") : locale === "ru" ? "несовместим" : "incompatible"}
                        </span>
                      )}
                    </span>
                    <span className="block truncate text-xs text-muted">
                      {(isBlocked || isIncompatible) && item.conflict_reason ? item.conflict_reason : item.description || (locale === "ru" ? "Без описания" : "No description")}
                    </span>
                  </button>
                  <span className="hidden truncate text-xs text-muted sm:block">{cat ? (locale === "ru" ? cat.ru : cat.en) : ""}</span>
                  <span className="hidden items-center gap-1 text-xs text-muted tabular-nums sm:flex">
                    <Star className="h-3 w-3" /> {item.stars}
                  </span>
                  <div className="flex justify-end">
                    {status.installed ? (
                      <Button size="sm" variant="ghost" onClick={() => setActiveTab("installed")} className="h-7 px-2.5 text-xs text-ok">
                        <CheckCircle2 className="mr-1 h-3.5 w-3.5" />
                        {locale === "ru" ? (status.enabled ? "Включён" : "Установлен") : status.enabled ? "Active" : "Installed"}
                      </Button>
                    ) : isBlocked || isIncompatible ? (
                      <Button size="sm" variant="ghost" onClick={() => setSelectedPublicModule(item)} className="h-7 px-2.5 text-xs text-muted">
                        {locale === "ru" ? "Почему" : "Why"}
                      </Button>
                    ) : (
                      <Button size="sm" variant="ghost" onClick={() => void handleBuildAndInstall(item.html_url)} disabled={building} className="h-7 border border-line px-3 text-xs">
                        {locale === "ru" ? "Собрать" : "Build"}
                      </Button>
                    )}
                  </div>
                </div>
              );
            })}
          </div>

          {filteredCommunityCatalog.length > communityDisplayLimit && (
            <div className="flex flex-col items-center gap-1.5 pt-1">
              <Button size="sm" variant="ghost" onClick={() => setCommunityDisplayLimit((prev) => prev + 50)} className="border border-line px-4 text-xs">
                {locale === "ru" ? "Показать ещё" : "Show more"}
              </Button>
              <span className="text-[11px] text-muted">
                {locale === "ru"
                  ? `Показано ${communityDisplayLimit} из ${filteredCommunityCatalog.length}`
                  : `Showing ${communityDisplayLimit} of ${filteredCommunityCatalog.length}`}
              </span>
            </div>
          )}

          {/* Details Modal for Verified Modules */}
          {selectedCatalogModule && (
            <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4" role="dialog">
              <Card className="w-full max-w-lg border-line bg-card p-6 shadow-2xl space-y-4">
                <div className="flex items-center justify-between border-b border-line pb-2">
                  <h3 className="font-semibold text-ink text-base flex items-center gap-2">
                    {selectedCatalogModule.name}
                    <span className="font-mono text-xs text-muted font-normal">({selectedCatalogModule.id})</span>
                  </h3>
                  <button onClick={() => setSelectedCatalogModule(null)} className="text-muted hover:text-ink">
                    <X className="h-4 w-4" />
                  </button>
                </div>

                <div className="space-y-3 text-xs">
                  <div>
                    <span className="text-muted block">{locale === "ru" ? "Описание:" : "Description:"}</span>
                    <p className="text-ink mt-0.5">
                      {locale === "ru" ? selectedCatalogModule.description.ru : selectedCatalogModule.description.en}
                    </p>
                  </div>

                  <div className="grid grid-cols-2 gap-2">
                    <div className="rounded bg-black/30 p-2">
                      <span className="text-muted block">{locale === "ru" ? "Уровень верификации:" : "Verification Level:"}</span>
                      <strong className="text-ink font-mono text-[11px]">{selectedCatalogModule.verificationLevel}</strong>
                    </div>
                    <div className="rounded bg-black/30 p-2">
                      <span className="text-muted block">{locale === "ru" ? "Уровень архитектуры:" : "Architecture Tier:"}</span>
                      <strong className="text-ink font-mono text-[11px]">{selectedCatalogModule.tier}</strong>
                    </div>
                  </div>

                  <div className="rounded border border-line bg-black/20 p-2.5">
                    <span className="text-muted block font-semibold mb-1">
                      {locale === "ru" ? "Инженерные примечания и статус:" : "Engineering Notes & Status:"}
                    </span>
                    <p className="text-zinc-300 leading-relaxed">
                      {locale === "ru" ? selectedCatalogModule.notes.ru : selectedCatalogModule.notes.en}
                    </p>
                  </div>
                </div>

                <div className="flex justify-end gap-2 pt-2 border-t border-line">
                  <Button size="sm" variant="ghost" onClick={() => void api.openLink(selectedCatalogModule.repo)} className="mr-auto">
                    <GithubMark className="mr-1.5 h-3.5 w-3.5" /> GitHub
                  </Button>
                  <Button size="sm" variant="ghost" onClick={() => setSelectedCatalogModule(null)}>
                    {locale === "ru" ? "Закрыть" : "Close"}
                  </Button>
                  {selectedCatalogModule.canInstall && (
                    <Button
                      size="sm"
                      variant="primary"
                      onClick={async () => {
                        const repo = selectedCatalogModule.repo;
                        const isOff = selectedCatalogModule.isOfficial;
                        const modId = selectedCatalogModule.id;
                        setSelectedCatalogModule(null);
                        if (isOff) {
                          try {
                            const m = modules?.find((x) => x.id === modId);
                            if (m && m.enabled) {
                              setOpenSettings(m.id);
                              setActiveTab("installed");
                              return;
                            }
                            await api.moduleSetEnabled(serverId, modId, true);
                            setChanged(true);
                            await load();
                            setOpenSettings(modId);
                            setActiveTab("installed");
                            onChanged?.();
                          } catch (e) {
                            setError(asUiError(e));
                          }
                        } else {
                          void handleBuildAndInstall(repo);
                        }
                      }}
                    >
                      <Hammer className="h-3.5 w-3.5 mr-1" />
                      {selectedCatalogModule.isOfficial
                        ? locale === "ru"
                          ? "Включить модуль"
                          : "Enable Module"
                        : locale === "ru"
                        ? "Перейти к сборке"
                        : "Proceed to Build"}
                    </Button>
                  )}
                </div>
              </Card>
            </div>
          )}

          {/* Details Modal for Public Community Modules */}
          {selectedPublicModule && (
            <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4" role="dialog">
              <Card className="w-full max-w-xl border-line bg-card p-6 shadow-2xl space-y-4">
                <div className="flex items-center justify-between border-b border-line pb-2">
                  <div>
                    <h3 className="font-semibold text-ink text-base flex items-center gap-2">
                      {selectedPublicModule.name}
                    </h3>
                    <span className="font-mono text-xs text-muted">{selectedPublicModule.full_name}</span>
                  </div>
                  <button onClick={() => setSelectedPublicModule(null)} className="text-muted hover:text-ink">
                    <X className="h-4 w-4" />
                  </button>
                </div>

                <div className="space-y-3 text-xs max-h-[70vh] overflow-y-auto pr-1">
                  <div>
                    <span className="text-muted block">{locale === "ru" ? "Описание:" : "Description:"}</span>
                    <p className="text-ink mt-0.5 leading-relaxed">
                      {selectedPublicModule.description || (locale === "ru" ? "Нет описания." : "No description.")}
                    </p>
                  </div>

                  {/* Metadata grid */}
                  <div className="grid grid-cols-2 sm:grid-cols-4 gap-2">
                    <div className="rounded bg-black/30 p-2 text-center">
                      <span className="text-muted block text-[10px]">{locale === "ru" ? "Звёзды" : "Stars"}</span>
                      <strong className="text-ink font-mono text-sm flex items-center justify-center gap-1 mt-0.5">
                        <Star className="h-3.5 w-3.5 text-gold fill-gold" />
                        {selectedPublicModule.stars}
                      </strong>
                    </div>

                    <div className="rounded bg-black/30 p-2 text-center">
                      <span className="text-muted block text-[10px]">{locale === "ru" ? "Форки" : "Forks"}</span>
                      <strong className="text-ink font-mono text-sm flex items-center justify-center gap-1 mt-0.5">
                        <GitFork className="h-3.5 w-3.5" />
                        {selectedPublicModule.forks}
                      </strong>
                    </div>

                    <div className="rounded bg-black/30 p-2 text-center">
                      <span className="text-muted block text-[10px]">{locale === "ru" ? "Лицензия" : "License"}</span>
                      <strong className="text-ink font-mono text-xs block truncate mt-1">
                        {selectedPublicModule.license || "—"}
                      </strong>
                    </div>

                    <div className="rounded bg-black/30 p-2 text-center">
                      <span className="text-muted block text-[10px]">{locale === "ru" ? "Обновлён" : "Pushed"}</span>
                      <strong className="text-ink font-mono text-xs block truncate mt-1">
                        {formatDate(selectedPublicModule.pushed_at)}
                      </strong>
                    </div>
                  </div>

                  {/* Compatibility status */}
                  <div className="rounded border border-line bg-black/20 p-3 space-y-2">
                    <div className="flex items-center justify-between">
                      <span className="text-muted font-semibold">
                        {locale === "ru" ? "Совместимость с Conquest of Azeroth:" : "CoA Compatibility Status:"}
                      </span>
                      <CoaStatusBadge status={selectedPublicModule.coa_status} conflictReason={selectedPublicModule.conflict_reason} />
                    </div>

                    {selectedPublicModule.conflict_reason && (
                      <div className="rounded bg-bad/10 border border-bad/30 p-2.5 text-bad text-xs">
                        <strong>{locale === "ru" ? "Причина блокировки / конфликта:" : "Conflict / Block Reason:"}</strong>
                        <p className="mt-1">{selectedPublicModule.conflict_reason}</p>
                      </div>
                    )}


                  </div>

                  {/* Topics */}
                  {selectedPublicModule.topics.length > 0 && (
                    <div>
                      <span className="text-muted block mb-1">{locale === "ru" ? "Темы (Topics):" : "Topics:"}</span>
                      <div className="flex flex-wrap gap-1">
                        {selectedPublicModule.topics.map((t) => (
                          <span key={t} className="rounded bg-white/5 border border-white/10 px-2 py-0.5 font-mono text-[11px] text-muted">
                            #{t}
                          </span>
                        ))}
                      </div>
                    </div>
                  )}
                </div>

                <div className="flex items-center justify-between pt-3 border-t border-line">
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() => void api.openLink(selectedPublicModule.html_url)}
                    className="text-xs"
                  >
                    <ExternalLink className="h-3.5 w-3.5 mr-1.5" />
                    {locale === "ru" ? "Открыть на GitHub" : "View on GitHub"}
                  </Button>

                  <div className="flex items-center gap-2">
                    <Button size="sm" variant="ghost" onClick={() => setSelectedPublicModule(null)}>
                      {locale === "ru" ? "Закрыть" : "Close"}
                    </Button>

                    {selectedPublicModule.coa_status !== "Blocked" && (
                      <Button
                        size="sm"
                        variant="primary"
                        onClick={() => {
                          const url = selectedPublicModule.html_url;
                          setSelectedPublicModule(null);
                          void handleBuildAndInstall(url);
                        }}
                        disabled={building}
                        className={cn(
                          "text-xs",
                          selectedPublicModule.coa_status === "Incompatible" && "bg-warn/80 hover:bg-warn text-black"
                        )}
                      >
                        <Hammer className="h-3.5 w-3.5 mr-1" />
                        {selectedPublicModule.coa_status === "Incompatible"
                          ? locale === "ru"
                            ? "Собрать (с риском)"
                            : "Build (Risky)"
                          : locale === "ru"
                          ? "Собрать и установить"
                          : "Build & Install"}
                      </Button>
                    )}
                  </div>
                </div>
              </Card>
            </div>
          )}
        </div>
      )}

      {/* TAB 4: BUILDS & MSVC LOGS */}
      {activeTab === "builds" && (
        <div className="space-y-4">
          {/* Active Build Progress Bar */}
          {building && buildProgress && (
            <Card className="border border-gold/40 bg-gold/5 p-4 space-y-3 shadow-lg">
              <div className="flex items-center justify-between text-xs">
                <div className="flex items-center gap-2">
                  <Loader2 className="h-4 w-4 animate-spin text-gold" />
                  <span className="font-semibold text-ink uppercase tracking-wider">{buildProgress.step}</span>
                  <span className="text-zinc-300">— {buildProgress.message}</span>
                </div>
                <div className="flex items-center gap-2 font-mono">
                  <span className="text-gold font-bold">{buildProgress.progress_pct}%</span>
                  <span className="text-zinc-400">({buildProgress.elapsedSeconds.toFixed(1)}s)</span>
                </div>
              </div>

              {/* Animated Progress Track */}
              <div className="h-2 w-full overflow-hidden rounded-full bg-black/60 border border-line/40">
                <div
                  className="h-full bg-gradient-to-r from-amber-500 via-yellow-400 to-amber-300 transition-all duration-300 ease-out"
                  style={{ width: `${Math.max(5, Math.min(100, buildProgress.progress_pct))}%` }}
                />
              </div>

              <div className="flex items-center justify-between text-[11px] text-zinc-400 pt-1 border-t border-line/30 font-sans">
                <span>{locale === "ru" ? "Используется Cl.exe & Ninja (MSVC x64 /MD)" : "Using Cl.exe & Ninja (MSVC x64 /MD)"}</span>
                <span>{locale === "ru" ? "Прогресс сборки и линковки DLL" : "DLL build and link progress"}</span>
              </div>
            </Card>
          )}

          {/* Build Result Card */}
          {buildResult && (
            <Card
              className={cn("border p-4", buildResult.success ? "border-ok/40 bg-ok/5" : "border-bad/40 bg-bad/5")}
            >
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-2">
                  {buildResult.success ? (
                    <CheckCircle2 className="h-5 w-5 text-ok" />
                  ) : (
                    <XCircle className="h-5 w-5 text-bad" />
                  )}
                  <div>
                    <h4 className="font-semibold text-ink text-sm">
                      {buildResult.display_name || buildResult.module_name} ({buildResult.duration_seconds.toFixed(1)}s)
                    </h4>
                    <p className="text-xs text-muted">
                      {buildResult.success
                        ? `DLL Qualified: ${buildResult.dll_sha256?.substring(0, 24)}... (x64 /MD)`
                        : `Build failed with ${buildResult.errors.length} diagnostic error(s)`}
                    </p>
                  </div>
                </div>

                {buildResult.success && (
                  <Button size="sm" variant="primary" onClick={handleInstallClick} disabled={installing}>
                    {locale === "ru" ? "Установить в реалм" : "Install to Realm"}
                  </Button>
                )}
              </div>

              {/* Error diagnostics */}
              {buildResult.errors.length > 0 && (
                <div className="mt-3 space-y-1">
                  <h5 className="text-xs font-semibold uppercase text-bad">MSVC Compiler / Linker Errors:</h5>
                  <ul className="space-y-1.5 text-xs">
                    {buildResult.errors.map((err, i) => (
                      <li key={i} className="rounded bg-black/40 p-2 font-mono text-bad">
                        <div>
                          [{err.error_code}] {err.file}:{err.line} - {err.message}
                        </div>
                        {err.actionable_resolution && (
                          <div className="mt-1 text-zinc-300 font-sans text-[11px]">
                            ➜ {err.actionable_resolution}
                          </div>
                        )}
                      </li>
                    ))}
                  </ul>
                </div>
              )}
            </Card>
          )}

          {/* Real-time Terminal Output */}
          <div className="rounded border border-line bg-black/80 p-4 font-mono text-xs text-zinc-300">
            <div className="flex items-center justify-between border-b border-zinc-800 pb-2 mb-2 text-zinc-400">
              <span className="flex items-center gap-2">
                <Terminal className="h-4 w-4" />
                MSVC / CMake Build Output
              </span>
              <span>{buildLogs.length} lines</span>
            </div>
            <pre className="max-h-96 overflow-y-auto whitespace-pre-wrap leading-relaxed select-text pr-2">
              {buildLogs.length === 0
                ? "No build logs recorded yet. Start a build from the 'GitHub' tab."
                : buildLogs.join("\n")}
            </pre>
          </div>
        </div>
      )}

      {/* TAB 5: TOOLCHAIN & ENVIRONMENT */}
      {activeTab === "toolchain" && toolchain && (
        <Card className="border-line bg-card/60 p-5 space-y-4">
          <h3 className="text-base font-semibold text-ink">
            {locale === "ru" ? "Среда компиляции и Plugin SDK" : "Compilation Environment & Plugin SDK"}
          </h3>
          <p className="text-xs text-muted">
            {locale === "ru"
              ? "Controlled Build Engine использует изолированные рецепты сборки, MSVC x64 и заголовки SDK v0.3. Исходные CMakeLists.txt модулей не исполняются."
              : "Controlled Build Engine uses isolated build recipes, MSVC x64 compiler, and SDK v0.3 headers. Upstream CMakeLists.txt is never executed."}
          </p>

          <div className="space-y-2 text-xs">
            <div className="flex items-center justify-between rounded border border-line p-2.5">
              <span>MSVC Toolchain (vcvars64.bat)</span>
              <span className="font-mono text-muted">{toolchain.vcvars64_path || "NOT FOUND"}</span>
            </div>
            <div className="flex items-center justify-between rounded border border-line p-2.5">
              <span>CMake Executable</span>
              <span className="font-mono text-muted">{toolchain.cmake_path || "NOT FOUND"}</span>
            </div>
            <div className="flex items-center justify-between rounded border border-line p-2.5">
              <span>Ninja Build Driver</span>
              <span className="font-mono text-muted">{toolchain.ninja_path || "NOT FOUND"}</span>
            </div>
            <div className="flex items-center justify-between rounded border border-line p-2.5">
              <span>CoA Plugin SDK Directory</span>
              <span className="font-mono text-muted">{toolchain.sdk_dir || "NOT FOUND"}</span>
            </div>
            <div className="flex items-center justify-between rounded border border-line p-2.5">
              <span>Host SDK ABI Fingerprint</span>
              <span className="font-mono text-muted">{toolchain.sdk_abi_fingerprint || "UNKNOWN"}</span>
            </div>
            <div className="flex items-center justify-between rounded border border-line p-2.5">
              <span>{locale === "ru" ? "Компилятор (нужен / выбран)" : "Compiler (required / selected)"}</span>
              <span className="font-mono text-muted">
                {toolchain.required_cl_version ? `cl ${toolchain.required_cl_version}` : "—"} / {toolchain.msvc_cl_version ? `cl ${toolchain.msvc_cl_version} (${toolchain.msvc_toolset ?? ""})` : "NOT FOUND"}
              </span>
            </div>
          </div>

          {!toolchain.ready && (
            <div className="space-y-2 rounded border border-line p-3 text-xs">
              <p className="text-muted">
                {locale === "ru"
                  ? "Модули собираются на этом ПК тем же компилятором, что и ядро (MSVC 14.44). Менеджер скачает официальный установщик Visual Studio Build Tools с сайта Microsoft, проверит подпись Microsoft и откроет его с уже отмеченными компонентами. Условия Microsoft вы принимаете в окне установщика. Нужно около 4 ГБ на диске и права администратора."
                  : "Modules are compiled on this PC with the same compiler as the core (MSVC 14.44). The Manager downloads the official Visual Studio Build Tools installer from Microsoft, checks Microsoft's signature and opens it with the components preselected. You accept Microsoft's terms in the installer window. About 4 GB of disk space and administrator rights are needed."}
              </p>
              <Button
                size="sm"
                disabled={msvcInstalling}
                onClick={async () => {
                  setMsvcInstalling(true);
                  setMsvcResult(null);
                  try {
                    const out = await api.previewInstallMsvc(serverId);
                    setMsvcResult(out);
                    setToolchain(out.toolchain);
                  } catch (e) {
                    setMsvcResult({ ready: false, restart_required: false, message: String(e) });
                  } finally {
                    setMsvcInstalling(false);
                  }
                }}
              >
                {msvcInstalling
                  ? (locale === "ru" ? "Идёт установка в окне Microsoft…" : "Installing in Microsoft's window…")
                  : (locale === "ru" ? "Установить Build Tools (MSVC 14.44)" : "Install Build Tools (MSVC 14.44)")}
              </Button>
              {msvcResult && (
                <p className={msvcResult.ready ? "text-ok" : "text-warn"} role="status">{msvcResult.message}</p>
              )}
            </div>
          )}
        </Card>
      )}

      {/* SQL Review & Confirmation Dialog Modal */}
      {showSqlDialog && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4" role="dialog" aria-modal="true">
          <Card className="w-full max-w-lg border-line bg-card p-6 shadow-2xl space-y-4">
            <div className="flex items-center gap-2 text-warn">
              <Database className="h-5 w-5" />
              <h3 className="text-base font-semibold">
                {locale === "ru" ? "Проверка SQL-миграций" : "SQL Migrations Review"}
              </h3>
            </div>
            <p className="text-xs text-muted">
              {locale === "ru"
                ? "Этот модуль содержит SQL-файлы данных или структуры. MySQL DDL не поддерживает откат через обычные транзакции."
                : "This module contains SQL structure or data files. MySQL DDL does not support rollback via standard transactions."}
            </p>

            {report && (
              <div className="space-y-3 py-2 text-xs">
                <div className="rounded border border-line/60 bg-black/30 p-2.5">
                  <span className="text-muted block font-semibold mb-1">
                    {locale === "ru" ? "Файлы миграций:" : "Migration Files:"}
                  </span>
                  <ul className="space-y-1 font-mono text-[11px] text-zinc-300">
                    {report.sql_files.map((s, idx) => (
                      <li key={idx}>
                        • {s.file_path} ({s.statement_count} statements)
                      </li>
                    ))}
                  </ul>
                </div>

                <div className="rounded border border-warn/30 bg-warn/10 p-2.5 text-warn flex items-start gap-2">
                  <AlertTriangle className="h-4 w-4 shrink-0 mt-0.5" />
                  <span>
                    {locale === "ru"
                      ? "Миграции будут применены к тестовой БД реалма. Удаление плагина сохранит таблицы нетронутыми."
                      : "Migrations will be applied to the realm test DB. Removing the plugin will preserve tables."}
                  </span>
                </div>
              </div>
            )}

            <div className="flex justify-end gap-2 pt-2 border-t border-line">
              <Button size="sm" variant="ghost" onClick={() => setShowSqlDialog(false)}>
                {locale === "ru" ? "Отмена" : "Cancel"}
              </Button>
              <Button size="sm" variant="primary" onClick={() => performInstall(true)}>
                {locale === "ru" ? "Подтвердить и применить SQL" : "Confirm and Apply SQL"}
              </Button>
            </div>
          </Card>
        </div>
      )}
    </div>
  );
}

/** Settings sub-component for official modules */
function ModuleSettings({
  serverId,
  module,
  onSaved,
  onRegisterGuard,
}: {
  serverId: string;
  module: ModuleView;
  onSaved: () => void;
  onRegisterGuard?: (guard: UnsavedChangesGuard | null) => void;
}) {
  const { t, tn, locale } = useI18n();
  const sx = useSchemaText();
  const human = useHuman();
  const [advanced, setAdvanced] = useState(false);
  const [items, setItems] = useState<ModuleSetting[] | null>(null);
  const [edits, setEdits] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<UiError | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [fieldErrors, setFieldErrors] = useState<Record<string, string>>({});

  useEffect(() => {
    let cancelled = false;
    setItems(null);
    setEdits({});
    setNote(null);
    setError(null);
    setFieldErrors({});
    void api
      .moduleSettings(serverId, module.id)
      .then((s) => {
        if (!cancelled) setItems(s);
      })
      .catch((e) => {
        if (!cancelled) setError(asUiError(e));
      });
    return () => {
      cancelled = true;
    };
  }, [serverId, module.id]);

  async function save(): Promise<boolean> {
    const errors: Record<string, string> = {};
    for (const [key, value] of Object.entries(edits)) {
      const field = moduleField(key, locale, items?.find((item) => item.key === key)?.field);
      if (field && ["int", "float", "range"].includes(field.type)) {
        const number = Number(value);
        const message =
          !value.trim() || !Number.isFinite(number)
            ? t("set.enterNumber")
            : field.type === "int" && !Number.isInteger(number)
            ? t("set.enterInt")
            : field.min !== undefined && number < field.min
            ? t("set.atLeast", { n: field.min })
            : field.max !== undefined && number > field.max
            ? t("set.atMost", { n: field.max })
            : null;
        if (message) errors[key] = message;
      }
    }
    setFieldErrors(errors);
    if (Object.keys(errors).length) return false;
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      const done = await api.moduleSaveSettings(serverId, module.id, edits);
      setEdits({});
      setItems(await api.moduleSettings(serverId, module.id));
      if (done.length) {
        onSaved();
        setNote(t("mod.saved", { n: done.length }));
      }
      return true;
    } catch (e) {
      setError(asUiError(e));
      return false;
    } finally {
      setBusy(false);
    }
  }

  const dirty = Object.keys(edits).length > 0;

  useEffect(() => {
    if (dirty) {
      onRegisterGuard?.({
        isDirty: () => Object.keys(edits).length > 0,
        save: () => save(),
        discard: () => {
          setEdits({});
          setFieldErrors({});
          setNote(null);
        },
      });
    } else {
      onRegisterGuard?.(null);
    }
    return () => onRegisterGuard?.(null);
  }, [dirty, edits, onRegisterGuard]);

  const edit = (key: string, value: string) =>
    setEdits((previous) => {
      const next = { ...previous };
      if (value === items?.find((item) => item.key === key)?.value) delete next[key];
      else next[key] = value;
      return next;
    });

  const available = items?.filter((s) => s.key !== moduleEnableKey(module.id) && !["CoACustomRaces.DisabledRaceIds", "CoACustomRaces.DisabledRaceCatalogSha256"].includes(s.key) && !retiredModuleKeys.has(s.key)) ?? [];
  const advancedCount = available.filter(
    (s) =>
      !auctionWeightKey(s.key) &&
      (!moduleField(s.key, locale, s.field) || moduleField(s.key, locale, s.field)?.advanced)
  ).length;

  const visible = available.filter((s) => {
    if (module.id === "ah-bot" && auctionWeightKey(s.key)) return false;
    const field = moduleField(s.key, locale, s.field);
    if (!advanced && !query.trim() && (!field || field.advanced)) return false;
    const title = field ? (s.field ? field.title : sx.title({ key: s.key, title: field.title })) : s.key;
    const description = field
      ? s.field
        ? field.description
        : sx.description({ key: s.key, description: field.description })
      : s.doc;
    return `${title} ${description} ${s.key}`.toLocaleLowerCase(locale).includes(query.trim().toLocaleLowerCase(locale));
  });

  return (
    <div className="mt-3 border-t border-line pt-3">
      {items === null && !error && <Loader2 className="h-4 w-4 animate-spin text-muted" aria-hidden />}
      {module.id === "npc-enchanter" && (
        <div className="mb-4 space-y-2 text-sm text-muted">
          <p>{t("mod.enchanter.add")}</p>
          <p>{t("mod.enchanter.remove")}</p>
        </div>
      )}
      {module.id === "ah-bot" && (
        <div className="mb-4 space-y-2 text-sm text-muted">
          <p>{t("mod.ahbot.setup")}</p>
        </div>
      )}
      {available.length >= 10 && (
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          aria-label={t("set.search")}
          placeholder={t("set.search")}
          className="mb-3 w-full rounded-md border border-line bg-bg px-3 py-2 text-sm outline-none focus:border-gold"
        />
      )}
      {advancedCount > 0 && (
        <label className="mb-3 flex items-center gap-2 text-sm text-muted">
          <input type="checkbox" checked={advanced} onChange={(e) => setAdvanced(e.target.checked)} />
          {t("set.advanced", { n: advancedCount })}
        </label>
      )}
      {module.id === "ah-bot" && (
        <AuctionTypes
          items={available}
          edits={edits}
          busy={busy}
          query={query}
          errors={fieldErrors}
          onChange={(changes) =>
            setEdits((previous) => {
              const next = { ...previous };
              for (const [key, value] of Object.entries(changes)) {
                if (value === items?.find((item) => item.key === key)?.value) delete next[key];
                else next[key] = value;
              }
              return next;
            })
          }
        />
      )}
      {items && !visible.length && !available.some((s) => module.id === "ah-bot" && auctionWeightKey(s.key)) && (
        <p className="py-3 text-sm text-muted">{t(available.length ? "all.none" : "mod.noSettings")}</p>
      )}
      <ul className="divide-y divide-line">
        {visible.map((s) => {
          const field = moduleField(s.key, locale, s.field);
          const title = field ? (s.field ? field.title : sx.title({ key: s.key, title: field.title })) : s.key;
          const description = field
            ? s.field
              ? field.description
              : sx.description({ key: s.key, description: field.description })
            : s.doc;
          const value = edits[s.key] ?? s.value;
          const normalized = canonicalModuleValue(s.key, value);
          const options = field?.options ?? [];
          const bool =
            field?.type === "bool" ||
            /^(true|false)$/i.test(normalized) ||
            (/^(0|1)$/.test(normalized) && /(?:\.Enable(?:d)?$|Enable\/Disable)/i.test(s.key + " " + s.doc));
          const on = /^(1|true|yes|on)$/i.test(normalized);
          const id = `${module.id}-${s.key}`;
          const controlClass =
            "selectable w-56 rounded-md border border-line bg-bg px-3 py-2 text-sm outline-none focus:border-gold";
          return (
            <li key={s.key} className="py-2.5">
              <div className="flex flex-wrap items-center justify-between gap-3">
                <div className="selectable text-sm font-medium">
                  <label htmlFor={id}>{title}</label>
                  {s.key in edits && <span className="ml-2 text-xs text-gold">{t("set.changed")}</span>}
                </div>
                {bool ? (
                  <button
                    id={id}
                    role="switch"
                    aria-checked={on}
                    disabled={busy}
                    onClick={() =>
                      edit(
                        s.key,
                        /^(true|false)$/i.test(canonicalModuleValue(s.key, s.value))
                          ? String(!on)
                          : on
                          ? "0"
                          : "1"
                      )
                    }
                    className={cn(
                      "relative h-7 w-14 cursor-pointer rounded-full",
                      on ? "bg-gold" : "bg-white/15"
                    )}
                  >
                    <span
                      className={cn(
                        "absolute left-1 top-1 h-5 w-5 rounded-full bg-white transition-transform",
                        on && "translate-x-7"
                      )}
                    />
                    <span className="sr-only">{t(on ? "set.on" : "set.off")}</span>
                  </button>
                ) : options.length ? (
                  <select
                    id={id}
                    value={normalized}
                    disabled={busy}
                    onChange={(e) => edit(s.key, e.target.value)}
                    className={controlClass}
                  >
                    {!options.some((o) => String(o.value) === normalized) && (
                      <option value={normalized}>{normalized}</option>
                    )}
                    {options.map((o) => (
                      <option key={String(o.value)} value={String(o.value)}>
                        {sx.option(s.key, o)}
                      </option>
                    ))}
                  </select>
                ) : field?.type === "range" ? (
                  <div className="flex w-56 items-center gap-3">
                    <input
                      id={id}
                      type="range"
                      min={field.min}
                      max={field.max}
                      step={field.step}
                      value={Number(normalized)}
                      disabled={busy}
                      onChange={(e) => edit(s.key, e.target.value)}
                      aria-valuetext={`${Math.round(Number(normalized) * 100)}%`}
                      className="min-w-0 flex-1 accent-gold"
                    />
                    <output htmlFor={id} className="w-12 text-right text-sm">
                      {Math.round(Number(normalized) * 100)}%
                    </output>
                  </div>
                ) : (
                  <input
                    id={id}
                    type={field?.type === "int" || field?.type === "float" ? "number" : "text"}
                    min={field?.min}
                    max={field?.max}
                    step={field?.type === "float" ? "any" : 1}
                    value={value}
                    disabled={busy}
                    onChange={(e) => edit(s.key, e.target.value)}
                    className={controlClass}
                  />
                )}
              </div>
              {description && <p className="mt-1 text-sm text-muted">{description}</p>}
              {fieldErrors[s.key] && (
                <p className="mt-1 text-sm text-bad" role="alert">
                  {fieldErrors[s.key]}
                </p>
              )}
              {advanced && <p className="mt-1 selectable text-xs text-muted/70">{s.key}</p>}
              {s.default !== null && (edits[s.key] ?? s.value) !== s.default && (
                <button
                  disabled={busy}
                  className="mt-1 cursor-pointer text-xs text-muted underline hover:text-ink"
                  onClick={() => edit(s.key, s.default ?? "")}
                >
                  {t("mod.default", { v: s.default })}
                </button>
              )}
            </li>
          );
        })}
      </ul>
      {available.length > 0 && (
        <div className="mt-3 flex items-center gap-3">
          <Button size="sm" variant="primary" disabled={!dirty || busy} onClick={() => void save()}>
            {busy && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
            {t("acc.save")}
          </Button>
          {dirty && (
            <Button
              size="sm"
              variant="ghost"
              disabled={busy}
              onClick={() => {
                setEdits({});
                setFieldErrors({});
                setNote(null);
              }}
            >
              {t("set.discard")}
            </Button>
          )}
          {note && <span className="text-sm text-ok" role="status">{note}</span>}
        </div>
      )}
      {error && (
        <p className="mt-2 text-sm text-bad" role="alert">
          {error.human ? human(error.human).message : error.technical}
        </p>
      )}
      {dirty && (
        <div className="fixed bottom-0 left-60 right-0 z-40 flex items-center justify-between border-t border-line bg-[#0b0c0e]/95 px-10 py-3 shadow-lg">
          <span className="text-sm text-muted">{tn("set.unsaved", Object.keys(edits).length)}</span>
          <div className="flex gap-2">
            <Button
              variant="ghost"
              size="sm"
              disabled={busy}
              onClick={() => {
                setEdits({});
                setFieldErrors({});
                setNote(null);
              }}
            >
              {t("set.discard")}
            </Button>
            <Button variant="primary" size="sm" disabled={busy} onClick={() => void save()}>
              {busy && <Loader2 className="mr-1.5 h-3.5 w-3.5 animate-spin" aria-hidden />}
              {t("acc.save")}
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}
