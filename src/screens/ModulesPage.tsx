import { Fragment, useCallback, useEffect, useRef, useState, type ComponentType, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  AlertTriangle,
  Bot,
  CheckCircle2,
  Database,
  Download,
  Flame,
  FolderPlus,
  Gavel,
  Hammer,
  Info,
  Layers,
  Loader2,
  Plug,
  Puzzle,
  Scaling,
  Search,
  Settings2,
  Shield,
  Snowflake,
  Sparkles,
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
  type CompatibilityReport,
  type CustomRacesClientStatus,
  type CustomRacesProgress,
  type InstalledPreviewPlugin,
  type ModuleSetting,
  type ModuleView,
  type ToolchainStatus,
  type UiError,
} from "@/lib/api";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { useHuman, useI18n, useSchemaText, type Key } from "@/i18n";
import { auctionWeightKey, canonicalModuleValue, moduleEnableKey, moduleField } from "@/lib/moduleSettings";
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
  qualification: "OFFICIAL_CORE" | "INSTALL_VERIFIED" | "LINK_VERIFIED" | "BUILT";
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
  // Official Core Modules
  {
    id: "companions",
    name: "CoA Companions",
    repo: "https://github.com/Corfirean/mod-coa-playerbots",
    category: "official",
    description: {
      ru: "Боты-компаньоны для групп, подземелий и гильдии.",
      en: "Companion bots for groups, dungeons and your guild.",
    },
    qualification: "OFFICIAL_CORE",
    tier: "Core Module",
    verificationLevel: "PRODUCTION_SUPPORTED",
    notes: {
      ru: "Встроенный официальный модуль сервера. Взаимоисключение со SQUID's Playerbots.",
      en: "Official built-in server module. Mutually exclusive with Playerbots.",
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
    qualification: "OFFICIAL_CORE",
    tier: "Core Module",
    verificationLevel: "PRODUCTION_SUPPORTED",
    notes: {
      ru: "Встроенный официальный модуль динамического масштабирования подземелий.",
      en: "Official built-in dynamic dungeon scaling module.",
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
    qualification: "OFFICIAL_CORE",
    tier: "Core Module",
    verificationLevel: "PRODUCTION_SUPPORTED",
    notes: {
      ru: "Встроенный пакет контента Burning Crusade.",
      en: "Built-in Burning Crusade content pack.",
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
    qualification: "OFFICIAL_CORE",
    tier: "Core Module",
    verificationLevel: "PRODUCTION_SUPPORTED",
    notes: {
      ru: "Встроенный пакет контента Wrath of the Lich King.",
      en: "Built-in Wrath of the Lich King content pack.",
    },
    sqlRequired: false,
    canInstall: true,
    isOfficial: true,
    icon: "wotlk",
  },
  {
    id: "playerbots",
    name: "SQUID's Playerbots",
    repo: "https://github.com/Zyth45/mod-playerbots",
    category: "official",
    description: {
      ru: "Альтернативная система ботов со случайными ботами и полным ИИ.",
      en: "An alternative bot system with random bots and full AI.",
    },
    qualification: "OFFICIAL_CORE",
    tier: "Core Module",
    verificationLevel: "PRODUCTION_SUPPORTED",
    notes: {
      ru: "Встроенная система случайных ботов SQUID. Взаимоисключение с CoA Companions.",
      en: "Built-in SQUID random bots system. Mutually exclusive with CoA Companions.",
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
    qualification: "OFFICIAL_CORE",
    tier: "Core Module",
    verificationLevel: "PRODUCTION_SUPPORTED",
    notes: {
      ru: "Встроенный модуль рас. Требуется установка клиентского MPQ-патча.",
      en: "Built-in custom races module. Requires client MPQ patch.",
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
    qualification: "OFFICIAL_CORE",
    tier: "Core Module",
    verificationLevel: "PRODUCTION_SUPPORTED",
    notes: {
      ru: "Встроенный модуль синхронизации игрового времени с сервером.",
      en: "Built-in world time synchronization module.",
    },
    sqlRequired: false,
    canInstall: true,
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
    repo: "https://github.com/Corfirean/azerothcore-wotlk-coa/tree/coa-bots/modules/mod-ah-bot-plus",
    category: "official",
    description: {
      ru: "Наполняет аукцион реалистичными настраиваемыми лотами.",
      en: "Fills the auction house with realistic, adjustable listings.",
    },
    qualification: "OFFICIAL_CORE",
    tier: "Core Module",
    verificationLevel: "PRODUCTION_SUPPORTED",
    notes: {
      ru: "Встроенный официальный модуль аукциона виртуальных персонажей.",
      en: "Built-in official virtual characters auction house module.",
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
    qualification: "OFFICIAL_CORE",
    tier: "Core Module",
    verificationLevel: "PRODUCTION_SUPPORTED",
    notes: {
      ru: "Встроенный официальный модуль зачарования экипировки у НПС.",
      en: "Built-in official NPC equipment enchanting module.",
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
  
  {
    id: "solocraft",
    name: "Solocraft",
    repo: "https://github.com/azerothcore/mod-solocraft",
    category: "scaling",
    description: {
      ru: "Масштабирование характеристик персонажа при одиночном прохождении группового контента.",
      en: "Dynamically boosts player stats in group dungeons and raids for solo play.",
    },
    qualification: "BUILT",
    tier: "Tier 1: Dynamic DLL",
    verificationLevel: "L0-L2 PASS",
    notes: {
      ru: "Сборка и связывание успешны. Параметры настраиваются через конфигурационный файл.",
      en: "Build and link successful. Configurable via settings.",
    },
    sqlRequired: true,
    canInstall: true,
    icon: "scaling",
  },
];

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
  const [catalogFilter, setCatalogFilter] = useState<string>("all");
  const [selectedCatalogModule, setSelectedCatalogModule] = useState<VerifiedCatalogModule | null>(null);

  // GitHub Intake & Analysis state
  const [githubUrl, setGithubUrl] = useState("");
  const [analyzing, setAnalyzing] = useState(false);
  const [analysisError, setAnalysisError] = useState<string | null>(null);
  const [report, setReport] = useState<CompatibilityReport | null>(null);

  // Toolchain state
  const [toolchain, setToolchain] = useState<ToolchainStatus | null>(null);

  // Controlled Build state
  const [building, setBuilding] = useState(false);
  const [buildResult, setBuildResult] = useState<BuildResult | null>(null);
  const [buildLogs, setBuildLogs] = useState<string[]>([]);

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

  const loadToolchain = useCallback(async () => {
    try {
      const tc = await api.previewToolchainStatus();
      setToolchain(tc);
    } catch (e) {
      console.error("Failed to load toolchain status", e);
    }
  }, []);

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
      // Mutual exclusion check between companions and squid
      if (m.id === "companions" && !m.enabled) {
        const squidMod = modules?.find((x) => x.id === "squid");
        if (squidMod?.enabled) {
          await api.moduleSetEnabled(serverId, "squid", false);
        }
      } else if (m.id === "squid" && !m.enabled) {
        const compMod = modules?.find((x) => x.id === "companions");
        if (compMod?.enabled) {
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

  async function registerTestRealm() {
    setActionInProgress("register-realm");
    setError(null);
    try {
      await api.previewRegisterTestRealm();
      setNotification(
        locale === "ru"
          ? "Экспериментальный тестовый реалм server-v03 успешно зарегистрирован!"
          : "Experimental server-v03 test realm registered successfully!"
      );
      onChanged?.();
      await load();
    } catch (e) {
      setError(asUiError(e));
    } finally {
      setActionInProgress(null);
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
      setAnalysisError(e instanceof Error ? e.message : String(e));
    } finally {
      setAnalyzing(false);
    }
  }

  async function startControlledBuild(urlToBuildParam?: string) {
    const target = (urlToBuildParam || (report ? report.repository_name : githubUrl)).trim();
    if (!target) return;
    setBuilding(true);
    setActiveTab("builds");
    setBuildLogs(["[CSM Controlled Build Engine] Initializing build environment..."]);
    try {
      setBuildLogs((prev) => [
        ...prev,
        `Target module: ${target}`,
        "Compiling C++ sources against CoA Plugin SDK v0.3 via MSVC...",
      ]);
      const res = await api.previewBuildModule(serverId, target);
      setBuildResult(res);
      setBuildLogs(res.log_output);
      if (res.success) {
        setNotification(
          locale === "ru"
            ? `Сборка успешно завершена! DLL '${res.display_name || res.module_id}' квалифицирована: ${res.dll_sha256?.substring(0, 16)}...`
            : `Build succeeded! DLL '${res.display_name || res.module_id}' qualified: ${res.dll_sha256?.substring(0, 16)}...`
        );
      } else {
        setAnalysisError(
          locale === "ru"
            ? "Сборка завершилась ошибкой. Проверьте диагностику компилятора во вкладке 'Сборка & логи MSVC'."
            : "Build failed. Check compiler diagnostics in the 'Builds & MSVC Logs' tab."
        );
      }
    } catch (e: unknown) {
      setAnalysisError(e instanceof Error ? e.message : String(e));
      setBuildLogs((prev) => [...prev, `[FATAL BUILD ERROR]: ${String(e)}`]);
    } finally {
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

  const installedModules = modules?.filter((m) => m.installed) ?? [];
  const selectedModule = modules?.find((m) => m.id === openSettings) ?? null;
  const selectedModuleIndex = installedModules.findIndex((m) => m.id === openSettings);

  const filteredCatalog = VERIFIED_CATALOG.filter((item) => {
    if (catalogFilter === "official" && !item.isOfficial) return false;
    if (catalogFilter === "verified" && item.isOfficial) return false;
    if (!catalogQuery.trim()) return true;
    const q = catalogQuery.toLowerCase();
    return (
      item.name.toLowerCase().includes(q) ||
      item.id.toLowerCase().includes(q) ||
      item.description.ru.toLowerCase().includes(q) ||
      item.description.en.toLowerCase().includes(q)
    );
  });

  return (
    <div className="flex h-full flex-col p-6 space-y-4 max-w-6xl">
      {/* Header & Sub-navigation */}
      <div className="flex flex-wrap items-center justify-between gap-4 border-b border-line pb-4">
        <div>
          <h2 className="text-xl font-bold tracking-tight text-ink flex items-center gap-2">
            <Plug className="h-6 w-6 text-gold" />
            {locale === "ru" ? "Модули и плагины" : "Modules & Plugins"}
            <span className="rounded bg-sky-500/20 px-2 py-0.5 text-xs font-mono font-medium text-sky-400">
              MODULES PREVIEW
            </span>
          </h2>
          <p className="mt-1 text-xs text-muted">
            {locale === "ru"
              ? "Автономная тестовая редакция для управления официальными модулями и установки сторонних модулей AzerothCore в виде DLL."
              : "Standalone preview for official module management and dynamic third-party DLL plugin installation."}
          </p>
        </div>

        {/* Global actions */}
        <div className="flex items-center gap-2">
          <Button
            size="sm"
            variant="ghost"
            onClick={exportDiagnostics}
            disabled={actionInProgress === "export-diag"}
            className="text-xs"
          >
            <Download className="mr-1.5 h-3.5 w-3.5" />
            {locale === "ru" ? "Экспорт отчёта" : "Export Diagnostics"}
          </Button>

          <Button
            size="sm"
            variant="ghost"
            onClick={registerTestRealm}
            disabled={actionInProgress === "register-realm"}
            className="text-xs text-gold hover:text-gold"
          >
            <FolderPlus className="mr-1.5 h-3.5 w-3.5" />
            {locale === "ru" ? "Подключить test-realm-v03" : "Register test-realm-v03"}
          </Button>
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

      {/* Toolchain Health Status Bar */}
      {toolchain && (
        <div className="grid grid-cols-2 gap-2 sm:grid-cols-4 text-xs rounded border border-line bg-black/20 p-2.5">
          <div className="flex items-center gap-2">
            {toolchain.msvc_available ? (
              <CheckCircle2 className="h-4 w-4 text-ok" />
            ) : (
              <XCircle className="h-4 w-4 text-bad" />
            )}
            <span>
              MSVC (vcvars64): <strong>{toolchain.msvc_available ? "Ready" : "Missing"}</strong>
            </span>
          </div>
          <div className="flex items-center gap-2">
            {toolchain.cmake_available ? (
              <CheckCircle2 className="h-4 w-4 text-ok" />
            ) : (
              <XCircle className="h-4 w-4 text-bad" />
            )}
            <span>
              CMake: <strong>{toolchain.cmake_available ? "Ready" : "Missing"}</strong>
            </span>
          </div>
          <div className="flex items-center gap-2">
            {toolchain.ninja_available ? (
              <CheckCircle2 className="h-4 w-4 text-ok" />
            ) : (
              <XCircle className="h-4 w-4 text-bad" />
            )}
            <span>
              Ninja: <strong>{toolchain.ninja_available ? "Ready" : "Missing"}</strong>
            </span>
          </div>
          <div className="flex items-center gap-2">
            {toolchain.sdk_available ? (
              <CheckCircle2 className="h-4 w-4 text-ok" />
            ) : (
              <XCircle className="h-4 w-4 text-bad" />
            )}
            <span>
              Plugin SDK v0.3: <strong>{toolchain.sdk_available ? "Ready" : "Missing"}</strong>
            </span>
          </div>
        </div>
      )}

      {/* GitHub URL Intake Bar on General Modules Screen */}
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
              onClick={async () => {
                if (!githubUrl.trim()) return;
                await runAnalysis();
                await startControlledBuild();
              }}
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
      </Card>

      {/* Tabs Header */}
      <div className="flex flex-wrap border-b border-line gap-2">
        <button
          onClick={() => setActiveTab("installed")}
          className={cn(
            "flex items-center gap-2 border-b-2 px-3.5 py-2 text-sm font-medium transition-colors",
            activeTab === "installed"
              ? "border-gold text-gold"
              : "border-transparent text-muted hover:text-ink"
          )}
        >
          <Plug className="h-4 w-4" />
          {locale === "ru" ? "Установленные модули" : "Installed Modules"}
          {installedModules.length > 0 && (
            <span className="rounded-full bg-gold/15 text-gold text-[11px] px-1.5 py-0.2 font-mono">
              {installedModules.length}
            </span>
          )}
        </button>

        <button
          onClick={() => setActiveTab("catalog")}
          className={cn(
            "flex items-center gap-2 border-b-2 px-3.5 py-2 text-sm font-medium transition-colors",
            activeTab === "catalog"
              ? "border-gold text-gold"
              : "border-transparent text-muted hover:text-ink"
          )}
        >
          <Layers className="h-4 w-4" />
          {locale === "ru" ? "Каталог модулей" : "Modules Catalog"}
          <span className="rounded-full bg-white/10 text-muted text-[11px] px-1.5 py-0.2 font-mono">
            {VERIFIED_CATALOG.length}
          </span>
        </button>

        <button
          onClick={() => setActiveTab("builds")}
          className={cn(
            "flex items-center gap-2 border-b-2 px-3.5 py-2 text-sm font-medium transition-colors",
            activeTab === "builds"
              ? "border-gold text-gold"
              : "border-transparent text-muted hover:text-ink"
          )}
        >
          <Terminal className="h-4 w-4" />
          {locale === "ru" ? "Сборка & логи MSVC" : "Builds & MSVC Logs"}
          {building && <Loader2 className="h-3.5 w-3.5 animate-spin text-gold" />}
        </button>

        <button
          onClick={() => setActiveTab("toolchain")}
          className={cn(
            "flex items-center gap-2 border-b-2 px-3.5 py-2 text-sm font-medium transition-colors",
            activeTab === "toolchain"
              ? "border-gold text-gold"
              : "border-transparent text-muted hover:text-ink"
          )}
        >
          <Shield className="h-4 w-4" />
          {locale === "ru" ? "Среда сборки & SDK" : "Environment & SDK"}
        </button>
      </div>

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
              const isDynamic = !OFFICIAL_IDS.has(m.id) && (installedPlugins.some((p) => p.id === m.id) || m.id.length === 40);
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
                          {isDynamic && (
                            <span className="rounded bg-sky-500/10 text-sky-400 border border-sky-500/30 px-1.5 py-0.2 text-[10px] font-mono uppercase">
                              DLL
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

                    {/* Custom Races Client Patch status & actions */}
                    {m.id === "custom-races" && (
                      <div className="w-full">
                        <p className="text-xs text-muted">
                          {locale === "ru"
                            ? "Установленная версия патча клиента: "
                            : "Installed client patch version: "}
                          {racesPatchStatus?.version ?? (locale === "ru" ? "не записана" : "not recorded")}
                        </p>
                        {racesInstalling && racesProgress ? (
                          <div className="rounded-lg border border-gold/30 bg-gold/5 p-3 mt-2">
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
                          <div className="flex flex-wrap items-center justify-between gap-2 border-t border-line/40 pt-3 text-xs mt-2">
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
                            <p className="w-full text-[11px] text-muted">
                              {locale === "ru"
                                ? m.enabled
                                  ? "После установки патч будет включён."
                                  : "После установки патч останется отключённым: модуль сервера выключен или отсутствует."
                                : m.enabled
                                ? "The patch will be enabled after installation."
                                : "The patch will stay disabled after installation: the server module is off or missing."}
                            </p>
                          </div>
                        )}
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
                        {!m.page && m.installed && m.has_settings && m.status !== "soon" && (
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
                      </div>

                      {isDynamic && (
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
                          <RaceAvailabilitySettings
                            serverId={serverId}
                            blocked={racesInstalling || toggling !== null}
                            onSaved={() => setChanged(true)}
                            onRegisterGuard={setRacesGuard}
                          />
                        )}
                      </SettingsPanel>
                    )}
                </Fragment>
              );
            })}
          </div>
        </div>
      )}

      {/* TAB 1: VERIFIED MODULES CATALOG */}
      {activeTab === "catalog" && (
        <div className="space-y-4">
          <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
            <div>
              <h3 className="text-sm font-semibold text-ink">
                {locale === "ru" ? "Каталог проверенных модулей" : "Verified Modules Catalog"}
              </h3>
              <p className="text-xs text-muted">
                {locale === "ru"
                  ? "Официальные модули и сторонние DLL, протестированные с платформой и готовые к работе."
                  : "Official modules and third-party DLL plugins verified and fully compatible with the platform."}
              </p>
            </div>

            {/* Category filter pills */}
            <div className="flex flex-wrap items-center gap-1.5 text-xs">
              <button
                onClick={() => setCatalogFilter("all")}
                className={cn(
                  "rounded-full px-2.5 py-1 transition-colors",
                  catalogFilter === "all" ? "bg-gold text-black font-semibold" : "bg-black/30 text-muted hover:text-ink"
                )}
              >
                {locale === "ru" ? "Все" : "All"} ({VERIFIED_CATALOG.length})
              </button>
              <button
                onClick={() => setCatalogFilter("official")}
                className={cn(
                  "rounded-full px-2.5 py-1 transition-colors",
                  catalogFilter === "official"
                    ? "bg-sky-400 text-black font-semibold"
                    : "bg-black/30 text-muted hover:text-ink"
                )}
              >
                {locale === "ru" ? "Официальные" : "Official Core"} ({VERIFIED_CATALOG.filter(c => c.isOfficial).length})
              </button>
              <button
                onClick={() => setCatalogFilter("verified")}
                className={cn(
                  "rounded-full px-2.5 py-1 transition-colors",
                  catalogFilter === "verified"
                    ? "bg-ok text-black font-semibold"
                    : "bg-black/30 text-muted hover:text-ink"
                )}
              >
                {locale === "ru" ? "Проверенные DLL" : "Verified DLLs"} ({VERIFIED_CATALOG.filter(c => !c.isOfficial).length})
              </button>
            </div>
          </div>

          {/* Search box */}
          <div className="relative">
            <Search className="absolute left-3 top-2.5 h-4 w-4 text-muted" />
            <input
              type="text"
              value={catalogQuery}
              onChange={(e) => setCatalogQuery(e.target.value)}
              placeholder={
                locale === "ru"
                  ? "Поиск по каталогу (например, buffer, enchanter, companions, scaling)..."
                  : "Search modules catalog..."
              }
              className="w-full rounded-md border border-line bg-bg pl-9 pr-4 py-2 text-xs text-ink placeholder:text-muted/60 focus:border-gold focus:outline-none"
            />
          </div>

          {/* Catalog Grid */}
          <div className="grid gap-3 sm:grid-cols-2">
            {filteredCatalog.map((item) => {
              const matchedModule = modules?.find(
                (m) => m.id === item.id || m.name.toLowerCase() === item.name.toLowerCase()
              );
              const isInstalled = Boolean(matchedModule?.installed);
              const isEnabled = Boolean(matchedModule?.enabled);

              return (
                <Card key={item.id} className="border-line bg-card p-4 flex flex-col justify-between space-y-3">
                  <div>
                    <div className="flex items-start justify-between gap-2">
                      <div className="flex items-start gap-3">
                        <Tile icon={item.icon || (item.isOfficial ? "bot" : "plug")} off={!isInstalled || !isEnabled} />
                        <div>
                          <h4 className="font-semibold text-ink text-sm flex items-center gap-2">
                            {item.name}
                            <span className="font-mono text-[11px] text-muted">({item.id})</span>
                          </h4>
                          <p className="text-xs text-muted mt-1">{locale === "ru" ? item.description.ru : item.description.en}</p>
                        </div>
                      </div>
                    </div>

                    {/* Qualification Badges */}
                    <div className="mt-3 flex flex-wrap items-center gap-1.5 text-[11px]">
                      {item.isOfficial ? (
                        <span className="rounded bg-sky-500/10 text-sky-400 border border-sky-500/30 px-2 py-0.5 font-medium">
                          ✓ OFFICIAL_CORE
                        </span>
                      ) : item.qualification === "INSTALL_VERIFIED" ? (
                        <span className="rounded bg-ok/10 text-ok border border-ok/30 px-2 py-0.5 font-medium">
                          ✓ INSTALL_VERIFIED
                        </span>
                      ) : item.qualification === "LINK_VERIFIED" ? (
                        <span className="rounded bg-sky-500/10 text-sky-400 border border-sky-500/30 px-2 py-0.5 font-medium">
                          ✓ LINK_VERIFIED
                        </span>
                      ) : (
                        <span className="rounded bg-teal-500/10 text-teal-300 border border-teal-500/30 px-2 py-0.5 font-medium">
                          ● BUILT
                        </span>
                      )}

                      <span className="rounded bg-white/5 text-muted px-2 py-0.5 font-mono">{item.tier}</span>
                      {item.sqlRequired && (
                        <span className="rounded bg-black/40 text-muted px-1.5 py-0.5 font-mono">SQL</span>
                      )}
                    </div>

                    <p className="mt-2 text-[11px] text-zinc-400 font-sans border-t border-line/40 pt-2">
                      {locale === "ru" ? item.notes.ru : item.notes.en}
                    </p>
                  </div>

                  <div className="flex items-center justify-between border-t border-line/60 pt-3">
                    <button
                      onClick={() => void api.openLink(item.repo)}
                      className="flex items-center gap-1.5 text-xs text-muted hover:text-ink"
                    >
                      <GithubMark className="h-3.5 w-3.5" />
                      <span>GitHub</span>
                    </button>

                    <div className="flex items-center gap-2">
                      <Button
                        size="sm"
                        variant="ghost"
                        onClick={() => setSelectedCatalogModule(item)}
                        className="text-xs h-7 px-2"
                      >
                        <Info className="h-3.5 w-3.5 mr-1" />
                        {locale === "ru" ? "Подробнее" : "Details"}
                      </Button>

                      {isInstalled ? (
                        <div className="flex items-center gap-1.5">
                          <span className="text-[11px] text-ok font-medium mr-1">
                            ✓ {locale === "ru" ? (isEnabled ? "Включен" : "Установлен") : (isEnabled ? "Active" : "Installed")}
                          </span>
                          <Button
                            size="sm"
                            variant="ghost"
                            onClick={() => {
                              if (matchedModule) {
                                setOpenSettings(matchedModule.id);
                              }
                              setActiveTab("installed");
                            }}
                            className="text-xs h-7 px-2.5 border border-gold/40 text-gold hover:bg-gold/10"
                          >
                            <Settings2 className="h-3.5 w-3.5 mr-1" />
                            {locale === "ru" ? "Настроить" : "Configure"}
                          </Button>
                        </div>
                      ) : item.isOfficial ? (
                        <Button
                          size="sm"
                          variant="primary"
                          onClick={async () => {
                            try {
                              const mod = modules?.find((m) => m.id === item.id);
                              if (mod && mod.enabled) {
                                setOpenSettings(mod.id);
                                setActiveTab("installed");
                                return;
                              }
                              await api.moduleSetEnabled(serverId, item.id, true);
                              setChanged(true);
                              await load();
                              setOpenSettings(item.id);
                              setActiveTab("installed");
                              onChanged?.();
                            } catch (e) {
                              setError(asUiError(e));
                            }
                          }}
                          className="text-xs h-7 px-3"
                        >
                          <Plug className="h-3.5 w-3.5 mr-1" />
                          {locale === "ru" ? "Включить" : "Enable"}
                        </Button>
                      ) : (
                        <Button
                          size="sm"
                          variant="primary"
                          onClick={async () => {
                            setGithubUrl(item.repo);
                            await runAnalysis(item.repo);
                            await startControlledBuild(item.repo);
                          }}
                          className="text-xs h-7 px-3"
                        >
                          <Hammer className="h-3.5 w-3.5 mr-1" />
                          {locale === "ru" ? "Собрать и установить" : "Build & Install"}
                        </Button>
                      )}
                    </div>
                  </div>
                </Card>
              );
            })}
          </div>

          {/* Details Modal */}
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
                          setGithubUrl(repo);
                          await runAnalysis(repo);
                          await startControlledBuild(repo);
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
        </div>
      )}

      {/* TAB 4: BUILDS & MSVC LOGS */}
      {activeTab === "builds" && (
        <div className="space-y-4">
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
          </div>
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

  const available = items?.filter((s) => s.key !== moduleEnableKey(module.id)) ?? [];
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
