import { useCallback, useEffect, useState } from "react";
import {
  AlertTriangle,
  CheckCircle2,
  Database,
  Download,
  FolderPlus,
  Hammer,
  Loader2,
  Plug,
  RefreshCw,
  Shield,
  Terminal,
  Trash2,
  X,
  XCircle,
} from "lucide-react";
import {
  api,
  asUiError,
  type BuildResult,
  type CompatibilityReport,
  type InstalledPreviewPlugin,
  type ToolchainStatus,
  type UiError,
} from "@/lib/api";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { useI18n } from "@/i18n";
import type { UnsavedChangesGuard } from "@/lib/unsavedGuard";

type TabId = "installed" | "github" | "builds" | "toolchain";

export function ModulesPage({
  serverId,
  onChanged,
  onRegisterGuard: _onRegisterGuard,
}: {
  serverId: string;
  onChanged?: () => void;
  onRegisterGuard?: (guard: UnsavedChangesGuard | null) => void;
}) {
  const { locale } = useI18n();

  // Navigation tab
  const [activeTab, setActiveTab] = useState<TabId>("github");

  // Installed dynamic plugins
  const [installedPlugins, setInstalledPlugins] = useState<InstalledPreviewPlugin[]>([]);
  const [error, setError] = useState<UiError | null>(null);
  const [notification, setNotification] = useState<string | null>(null);

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

  const load = useCallback(async () => {
    try {
      await loadInstalledPlugins();
      await loadToolchain();
    } catch (e) {
      setError(asUiError(e));
    }
  }, [loadInstalledPlugins, loadToolchain]);

  useEffect(() => {
    void load();
  }, [load]);

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
        locale === "ru"
          ? `Диагностический отчёт сохранён: ${p}`
          : `Diagnostic report saved: ${p}`
      );
    } catch (e) {
      setError(asUiError(e));
    } finally {
      setActionInProgress(null);
    }
  }

  async function runAnalysis() {
    if (!githubUrl.trim()) return;
    setAnalyzing(true);
    setAnalysisError(null);
    setReport(null);
    setBuildResult(null);
    try {
      const rep = await api.analyzeGithubModule(githubUrl.trim(), serverId);
      setReport(rep);
    } catch (e: unknown) {
      setAnalysisError(e instanceof Error ? e.message : String(e));
    } finally {
      setAnalyzing(false);
    }
  }

  async function startControlledBuild() {
    if (!githubUrl.trim() && !report) return;
    setBuilding(true);
    setActiveTab("builds");
    setBuildLogs(["[CSM Controlled Build Engine] Initializing build environment..."]);
    try {
      const urlToBuild = report ? report.repository_name : githubUrl.trim();
      setBuildLogs((prev) => [
        ...prev,
        `Target module: ${urlToBuild}`,
        "Compiling C++ sources against CoA Plugin SDK v0.3 via MSVC...",
      ]);
      const res = await api.previewBuildModule(serverId, githubUrl.trim());
      setBuildResult(res);
      setBuildLogs(res.log_output);
      if (res.success) {
        setNotification(
          locale === "ru"
            ? `Сборка успешно завершена! DLL квалифицирована: ${res.dll_sha256?.substring(0, 16)}...`
            : `Build succeeded! Qualified DLL: ${res.dll_sha256?.substring(0, 16)}...`
        );
      } else {
        setAnalysisError(
          locale === "ru"
            ? "Сборка завершилась ошибкой. Проверьте диагностику компилятора во вкладке 'Сборки'."
            : "Build failed. Check compiler diagnostics in the 'Builds' tab."
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
      await loadInstalledPlugins();
      setActiveTab("installed");
      onChanged?.();
    } catch (e) {
      setError(asUiError(e));
    } finally {
      setInstalling(false);
    }
  }

  async function togglePlugin(moduleId: string, currentEnabled: boolean) {
    setActionInProgress(`toggle-${moduleId}`);
    setError(null);
    try {
      const msg = await api.previewToggleModule(serverId, moduleId, !currentEnabled);
      setNotification(msg);
      await loadInstalledPlugins();
      onChanged?.();
    } catch (e) {
      setError(asUiError(e));
    } finally {
      setActionInProgress(null);
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
              ? "Автономная тестовая редакция для установки сторонних AzerothCore-модулей в виде динамических DLL."
              : "Standalone experimental preview for installing third-party AzerothCore modules as independent DLLs."}
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
          <span>⚠ {error.technical || error.human.message}</span>
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

      {/* Tabs Header */}
      <div className="flex border-b border-line gap-2">
        <button
          onClick={() => setActiveTab("github")}
          className={cn(
            "flex items-center gap-2 border-b-2 px-4 py-2 text-sm font-medium transition-colors",
            activeTab === "github"
              ? "border-gold text-gold"
              : "border-transparent text-muted hover:text-ink"
          )}
        >
          <Hammer className="h-4 w-4" />
          {locale === "ru" ? "Установка с GitHub" : "Install from GitHub"}
        </button>

        <button
          onClick={() => setActiveTab("installed")}
          className={cn(
            "flex items-center gap-2 border-b-2 px-4 py-2 text-sm font-medium transition-colors",
            activeTab === "installed"
              ? "border-gold text-gold"
              : "border-transparent text-muted hover:text-ink"
          )}
        >
          <Plug className="h-4 w-4" />
          {locale === "ru" ? "Установленные плагины" : "Installed Plugins"} ({installedPlugins.length})
        </button>

        <button
          onClick={() => setActiveTab("builds")}
          className={cn(
            "flex items-center gap-2 border-b-2 px-4 py-2 text-sm font-medium transition-colors",
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
            "flex items-center gap-2 border-b-2 px-4 py-2 text-sm font-medium transition-colors",
            activeTab === "toolchain"
              ? "border-gold text-gold"
              : "border-transparent text-muted hover:text-ink"
          )}
        >
          <Shield className="h-4 w-4" />
          {locale === "ru" ? "Среда сборки & SDK" : "Environment & SDK"}
        </button>
      </div>

      {/* TAB 1: GITHUB INSTALLATION & COMPATIBILITY WORKFLOW */}
      {activeTab === "github" && (
        <div className="space-y-4">
          <Card className="border-line bg-card/60 p-4">
            <h3 className="text-sm font-semibold text-ink">
              {locale === "ru" ? "1. Анализ и инспекция модуля" : "1. Module Intake & Analysis"}
            </h3>
            <p className="mt-1 text-xs text-muted">
              {locale === "ru"
                ? "Укажите ссылку на GitHub-репозиторий совместимого модуля AzerothCore (например, mod-congrats-on-level, mod-npc-buffer, mod-npc-enchanter, mod-reward-played-time)."
                : "Paste GitHub repository URL for an AzerothCore module (e.g., mod-congrats-on-level, mod-npc-buffer, mod-npc-enchanter, mod-reward-played-time)."}
            </p>

            <div className="mt-3 flex gap-2">
              <input
                type="text"
                value={githubUrl}
                onChange={(e) => setGithubUrl(e.target.value)}
                placeholder="https://github.com/azerothcore/mod-congrats-on-level"
                className="flex-1 rounded border border-line bg-bg px-3 py-1.5 text-xs text-ink placeholder:text-muted/50 focus:border-gold focus:outline-none"
              />
              <Button size="sm" variant="primary" onClick={runAnalysis} disabled={analyzing || !githubUrl.trim()}>
                {analyzing ? (
                  <>
                    <Loader2 className="mr-1.5 h-3.5 w-3.5 animate-spin" />
                    {locale === "ru" ? "Анализ..." : "Analyzing..."}
                  </>
                ) : (
                  locale === "ru" ? "Анализировать" : "Analyze"
                )}
              </Button>
            </div>

            {analysisError && (
              <div className="mt-3 rounded border border-bad/30 bg-bad/10 p-2.5 text-xs text-bad">
                {analysisError}
              </div>
            )}
          </Card>

          {/* Analysis Report Card */}
          {report && (
            <Card className="border-line bg-card/80 p-5 space-y-4">
              <div className="flex flex-wrap items-start justify-between gap-3 border-b border-line pb-3">
                <div>
                  <h3 className="text-base font-semibold text-ink flex items-center gap-2">
                    {report.repository_name}
                    <span className="text-xs font-mono font-normal text-muted">
                      ({report.commit.substring(0, 8)})
                    </span>
                  </h3>
                  <div className="mt-1 flex flex-wrap gap-2 text-xs">
                    <span className="rounded bg-black/40 px-2 py-0.5 text-muted">
                      License: <strong className="text-ink">{report.license_info.primary_license}</strong>
                    </span>
                    <span className="rounded bg-black/40 px-2 py-0.5 text-muted">
                      SDK Host:{" "}
                      <strong className="text-ink">
                        {report.sdk_fingerprint ? report.sdk_fingerprint.substring(0, 16) + "..." : "Default"}
                      </strong>
                    </span>
                  </div>
                </div>

                {/* Build and Install Buttons */}
                <div className="flex items-center gap-2">
                  <Button
                    size="sm"
                    variant="primary"
                    onClick={startControlledBuild}
                    disabled={building || report.classification === "CorePatchRequired"}
                  >
                    {building ? (
                      <>
                        <Loader2 className="mr-1.5 h-3.5 w-3.5 animate-spin" />
                        {locale === "ru" ? "Сборка..." : "Building..."}
                      </>
                    ) : (
                      <>
                        <Hammer className="mr-1.5 h-3.5 w-3.5" />
                        {locale === "ru" ? "Собрать модуль" : "Build Module"}
                      </>
                    )}
                  </Button>

                  <Button
                    size="sm"
                    variant="primary"
                    onClick={handleInstallClick}
                    disabled={installing || !buildResult || !buildResult.success}
                    className={cn(buildResult?.success ? "bg-ok hover:bg-ok/90" : "opacity-50 cursor-not-allowed")}
                  >
                    {installing ? (
                      <Loader2 className="h-3.5 w-3.5 animate-spin" />
                    ) : (
                      locale === "ru" ? "Установить в реалм" : "Install to Realm"
                    )}
                  </Button>
                </div>
              </div>

              {/* Multi-Dimensional Status */}
              {report.multi_dimensional_status && (
                <div className="grid grid-cols-2 gap-3 sm:grid-cols-5 text-center">
                  <div className="rounded border border-line bg-bg/50 p-2.5">
                    <div className="text-xs text-muted">{locale === "ru" ? "Сборка" : "Build"}</div>
                    <div
                      className={cn(
                        "mt-1 text-xs font-semibold uppercase",
                        report.multi_dimensional_status.build_status === "PASS" ? "text-ok" : "text-warn"
                      )}
                    >
                      {report.multi_dimensional_status.build_status}
                    </div>
                  </div>
                  <div className="rounded border border-line bg-bg/50 p-2.5">
                    <div className="text-xs text-muted">{locale === "ru" ? "Рантайм" : "Runtime"}</div>
                    <div
                      className={cn(
                        "mt-1 text-xs font-semibold uppercase",
                        report.multi_dimensional_status.runtime_status === "PASS" ? "text-ok" : "text-muted"
                      )}
                    >
                      {report.multi_dimensional_status.runtime_status}
                    </div>
                  </div>
                  <div className="rounded border border-line bg-bg/50 p-2.5">
                    <div className="text-xs text-muted">{locale === "ru" ? "Геймплей" : "Gameplay"}</div>
                    <div
                      className={cn(
                        "mt-1 text-xs font-semibold uppercase",
                        report.multi_dimensional_status.gameplay_status === "CLEAN" ? "text-ok" : "text-warn"
                      )}
                    >
                      {report.multi_dimensional_status.gameplay_status}
                    </div>
                  </div>
                  <div className="rounded border border-line bg-bg/50 p-2.5">
                    <div className="text-xs text-muted">{locale === "ru" ? "БД / SQL" : "Database"}</div>
                    <div
                      className={cn(
                        "mt-1 text-xs font-semibold uppercase",
                        report.multi_dimensional_status.db_status === "CLEAN" ? "text-ok" : "text-warn"
                      )}
                    >
                      {report.multi_dimensional_status.db_status}
                    </div>
                  </div>
                  <div className="rounded border border-line bg-bg/50 p-2.5">
                    <div className="text-xs text-muted">{locale === "ru" ? "Безопасность" : "Security"}</div>
                    <div
                      className={cn(
                        "mt-1 text-xs font-semibold uppercase",
                        report.multi_dimensional_status.security_status === "SOURCE_ANALYZED" ||
                          report.multi_dimensional_status.security_status === "REVIEWED_THIRD_PARTY" ||
                          report.multi_dimensional_status.security_status === "TRUSTED_OFFICIAL"
                          ? "text-ok"
                          : "text-bad"
                      )}
                    >
                      {report.multi_dimensional_status.security_status}
                    </div>
                  </div>
                </div>
              )}

              {/* Findings List */}
              <div>
                <h4 className="text-xs font-semibold uppercase tracking-wider text-muted">
                  {locale === "ru" ? "Диагностические находки" : "Diagnostic Findings"} ({report.findings.length})
                </h4>
                {report.findings.length === 0 ? (
                  <p className="mt-1 text-xs text-ok">✓ Замечаний и критических блокировок не обнаружено.</p>
                ) : (
                  <ul className="mt-2 space-y-2 max-h-48 overflow-y-auto pr-1">
                    {report.findings.map((f, idx) => (
                      <li key={idx} className="rounded border border-line/60 bg-black/20 p-2 text-xs">
                        <div className="flex items-center justify-between gap-2">
                          <span
                            className={cn(
                              "font-medium",
                              f.severity === "Critical"
                                ? "text-bad"
                                : f.severity === "Warning"
                                ? "text-warn"
                                : "text-sky-300"
                            )}
                          >
                            [{f.severity}] {f.rule_id}
                          </span>
                          <span className="font-mono text-muted">
                            {f.file}:{f.line}
                          </span>
                        </div>
                        <p className="mt-1 text-ink">{f.explanation}</p>
                        <p className="mt-0.5 text-muted">➜ {f.recommended_action}</p>
                      </li>
                    ))}
                  </ul>
                )}
              </div>
            </Card>
          )}
        </div>
      )}

      {/* TAB 2: INSTALLED PLUGINS */}
      {activeTab === "installed" && (
        <div className="space-y-4">
          <div className="flex items-center justify-between">
            <h3 className="text-sm font-semibold text-ink">
              {locale === "ru" ? "Динамические плагины в реалме" : "Dynamic Plugins in Realm"}
            </h3>
            <Button size="sm" variant="ghost" onClick={loadInstalledPlugins} className="text-xs">
              <RefreshCw className="mr-1.5 h-3.5 w-3.5" />
              {locale === "ru" ? "Обновить" : "Refresh"}
            </Button>
          </div>

          {installedPlugins.length === 0 ? (
            <Card className="border-line bg-card/40 p-8 text-center text-muted">
              <Plug className="mx-auto h-8 w-8 text-muted/50 mb-2" />
              <p className="text-sm">
                {locale === "ru"
                  ? "В этом реалме ещё нет установленных динамических плагинов."
                  : "No dynamic plugins installed in this realm yet."}
              </p>
              <Button size="sm" variant="primary" onClick={() => setActiveTab("github")} className="mt-4">
                {locale === "ru" ? "Установить первый модуль с GitHub" : "Install First Module from GitHub"}
              </Button>
            </Card>
          ) : (
            <div className="grid gap-3 sm:grid-cols-2">
              {installedPlugins.map((p) => (
                <Card key={p.id} className="border-line bg-card p-4 space-y-3">
                  <div className="flex items-start justify-between">
                    <div>
                      <h4 className="font-semibold text-ink text-sm flex items-center gap-2">
                        {p.name}
                        <span className="text-xs font-mono font-normal text-muted">v{p.version}</span>
                      </h4>
                      <p className="text-xs font-mono text-muted mt-0.5">
                        SHA256: {p.sha256.substring(0, 16)}...
                      </p>
                    </div>

                    <div className="flex items-center gap-2">
                      <span className={cn("text-xs font-semibold uppercase", p.enabled ? "text-ok" : "text-muted")}>
                        {p.enabled
                          ? locale === "ru"
                            ? "Включён"
                            : "Enabled"
                          : locale === "ru"
                          ? "Отключён"
                          : "Disabled"}
                      </span>
                      <button
                        type="button"
                        role="switch"
                        aria-checked={p.enabled}
                        onClick={() => togglePlugin(p.id, p.enabled)}
                        disabled={actionInProgress === `toggle-${p.id}`}
                        className={cn(
                          "relative inline-flex h-5 w-9 shrink-0 cursor-pointer rounded-full border-2 border-transparent transition-colors duration-200 ease-in-out focus:outline-none",
                          p.enabled ? "bg-ok" : "bg-zinc-700"
                        )}
                      >
                        <span
                          className={cn(
                            "pointer-events-none inline-block h-4 w-4 transform rounded-full bg-white shadow ring-0 transition duration-200 ease-in-out",
                            p.enabled ? "translate-x-4" : "translate-x-0"
                          )}
                        />
                      </button>
                    </div>
                  </div>

                  <div className="flex items-center justify-between border-t border-line/60 pt-2 text-xs">
                    <span className="text-muted">
                      {p.conf_files.length > 0 ? `${p.conf_files.length} config file(s)` : "No configs"}
                    </span>
                    <Button
                      size="sm"
                      variant="ghost"
                      onClick={() => removePlugin(p.id)}
                      disabled={actionInProgress === `remove-${p.id}`}
                      className="text-bad hover:text-bad hover:bg-bad/10 h-7 px-2"
                    >
                      <Trash2 className="h-3.5 w-3.5 mr-1" />
                      {locale === "ru" ? "Удалить" : "Remove"}
                    </Button>
                  </div>
                </Card>
              ))}
            </div>
          )}
        </div>
      )}

      {/* TAB 3: BUILDS & MSVC LOGS */}
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
                      {buildResult.module_name} ({buildResult.duration_seconds.toFixed(1)}s)
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
                  <ul className="space-y-1 text-xs">
                    {buildResult.errors.map((err, i) => (
                      <li key={i} className="rounded bg-black/40 p-2 font-mono text-bad">
                        [{err.error_code}] {err.file}:{err.line} - {err.message}
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

      {/* TAB 4: TOOLCHAIN & ENVIRONMENT */}
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
                      <li key={idx}>• {s.file_path} ({s.statement_count} statements)</li>
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
