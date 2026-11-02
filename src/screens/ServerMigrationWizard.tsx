import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  AlertCircle,
  AlertTriangle,
  ArrowRight,
  CheckCircle2,
  Database,
  FileCheck,
  FolderOpen,
  Loader2,
  RotateCcw,
  Server,
  Shield,
  ShieldCheck,
  Users,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import {
  api,
  type ConflictResolutionPolicy,
  type MigrationExecutionReport,
  type MigrationPlan,
  type MigrationScope,
  type SourceInspectionReport,
} from "@/lib/api";
import { cn } from "@/lib/utils";

export interface ServerMigrationWizardProps {
  currentRealmId: string;
  onClose?: () => void;
  onSuccess?: () => void;
}

type WizardStep = "source" | "inspect" | "scope" | "plan" | "executing" | "result";

export function ServerMigrationWizard({
  currentRealmId,
  onClose,
  onSuccess,
}: ServerMigrationWizardProps) {
  const locale = "ru";

  const [step, setStep] = useState<WizardStep>("source");
  const [sourcePath, setSourcePath] = useState("");
  const [inspecting, setInspecting] = useState(false);
  const [report, setReport] = useState<SourceInspectionReport | null>(null);

  // Scope
  const [scope, setScope] = useState<MigrationScope>({
    accounts_and_characters: true,
    module_owned_data: true,
    server_configuration: true,
    selected_modules: [],
  });

  // Conflict policy
  const [conflictPolicy, setConflictPolicy] = useState<ConflictResolutionPolicy>(
    "remap_colliding_guids"
  );

  // Plan & Execution
  const [planning, setPlanning] = useState(false);
  const [plan, setPlan] = useState<MigrationPlan | null>(null);
  const [executing, setExecuting] = useState(false);
  const [execReport, setExecReport] = useState<MigrationExecutionReport | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  async function handlePickSource() {
    setErrorMsg(null);
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: locale === "ru" ? "Выберите каталог исходного сервера" : "Select Source Server Directory",
      });
      if (typeof selected === "string") {
        setSourcePath(selected);
      }
    } catch (e) {
      setErrorMsg(String(e));
    }
  }

  async function handleInspect() {
    if (!sourcePath.trim()) return;
    setErrorMsg(null);
    setInspecting(true);
    try {
      const res = await api.previewMigrationInspect(sourcePath);
      setReport(res);
      // Auto-select detected modules
      setScope((prev) => ({
        ...prev,
        selected_modules: res.detected_modules.map((m) => m.id),
      }));
      setStep("inspect");
    } catch (e: any) {
      setErrorMsg(e?.human?.message || e?.technical || String(e));
    } finally {
      setInspecting(false);
    }
  }

  async function handleCreatePlan() {
    if (!report) return;
    setErrorMsg(null);
    setPlanning(true);
    try {
      const res = await api.previewMigrationPlan(sourcePath, currentRealmId, scope, conflictPolicy);
      setPlan(res);
      setStep("plan");
    } catch (e: any) {
      setErrorMsg(e?.human?.message || e?.technical || String(e));
    } finally {
      setPlanning(false);
    }
  }

  async function handleExecute() {
    if (!plan) return;
    setErrorMsg(null);
    setExecuting(true);
    setStep("executing");
    try {
      const res = await api.previewMigrationExecute(plan);
      setExecReport(res);
      setStep("result");
      if (res.success && onSuccess) {
        onSuccess();
      }
    } catch (e: any) {
      setErrorMsg(e?.human?.message || e?.technical || String(e));
      setStep("plan");
    } finally {
      setExecuting(false);
    }
  }

  return (
    <div className="flex h-full flex-col max-w-4xl mx-auto p-6 space-y-6">
      {/* Header */}
      <div className="flex items-center justify-between border-b border-line pb-4">
        <div>
          <h2 className="text-xl font-bold tracking-tight text-ink flex items-center gap-2">
            <RotateCcw className="h-6 w-6 text-gold" />
            {locale === "ru" ? "Центр переноса сервера" : "Server Migration Center"}
            <span className="rounded bg-sky-500/20 px-2 py-0.5 text-xs font-mono font-medium text-sky-400">
              MIGRATION WIZARD
            </span>
          </h2>
          <p className="mt-1 text-xs text-muted">
            {locale === "ru"
              ? "Перенос персонажей, аккаунтов, данных модулей и конфигураций в изолированный модульный сервер Preview."
              : "Migrate characters, accounts, module data and configs into an isolated modular Preview realm."}
          </p>
        </div>
        {onClose && (
          <Button size="sm" variant="ghost" onClick={onClose} className="text-xs">
            {locale === "ru" ? "Закрыть" : "Close"}
          </Button>
        )}
      </div>

      {errorMsg && (
        <Card className="border-bad/40 bg-bad/5 p-4 text-xs text-bad flex items-start gap-2.5">
          <AlertCircle className="h-4 w-4 shrink-0 mt-0.5" />
          <div className="flex-1">
            <p className="font-semibold">{locale === "ru" ? "Ошибка переноса" : "Migration Error"}</p>
            <p className="mt-0.5 whitespace-pre-wrap">{errorMsg}</p>
          </div>
        </Card>
      )}

      {/* Stepper Progress */}
      <div className="flex items-center justify-between text-xs font-medium text-muted px-2">
        <span className={cn(step === "source" && "text-gold font-bold")}>1. Исходный сервер</span>
        <ArrowRight className="h-3 w-3 text-line" />
        <span className={cn(step === "inspect" && "text-gold font-bold")}>2. Анализ данных</span>
        <ArrowRight className="h-3 w-3 text-line" />
        <span className={cn(step === "scope" && "text-gold font-bold")}>3. Область переноса</span>
        <ArrowRight className="h-3 w-3 text-line" />
        <span className={cn(step === "plan" && "text-gold font-bold")}>4. План и риски</span>
        <ArrowRight className="h-3 w-3 text-line" />
        <span className={cn((step === "executing" || step === "result") && "text-gold font-bold")}>5. Перенос</span>
      </div>

      {/* STEP 1: SELECT SOURCE */}
      {step === "source" && (
        <Card className="p-6 space-y-4">
          <h3 className="text-sm font-semibold text-ink flex items-center gap-2">
            <FolderOpen className="h-4 w-4 text-gold" />
            {locale === "ru" ? "Шаг 1: Укажите каталог исходного сервера" : "Step 1: Select Source Server"}
          </h3>
          <p className="text-xs text-muted">
            {locale === "ru"
              ? "Поддерживаются серверы под управлением CoA Server Manager, устаревшие CoA-серверы со встроенными ботами, а также стандартные серверы AzerothCore."
              : "Supports CSM-managed servers, legacy embedded CoA installations, and standard AzerothCore."}
          </p>

          <div className="flex items-center gap-3">
            <input
              type="text"
              value={sourcePath}
              onChange={(e) => setSourcePath(e.target.value)}
              placeholder="C:\games\CoA Server или путь к серверу..."
              className="flex-1 rounded-md border border-line bg-bg px-3 py-2 text-xs text-ink focus:border-gold focus:outline-none font-mono"
            />
            <Button size="sm" variant="secondary" onClick={handlePickSource}>
              <FolderOpen className="h-3.5 w-3.5 mr-1.5" />
              {locale === "ru" ? "Обзор..." : "Browse..."}
            </Button>
          </div>

          <div className="rounded-lg bg-white/5 border border-line p-3 text-xs text-muted space-y-1">
            <p className="font-semibold text-ink flex items-center gap-1.5">
              <ShieldCheck className="h-4 w-4 text-ok" />
              {locale === "ru" ? "Гарантия безопасности данных" : "Safety Guarantee"}
            </p>
            <p>
              {locale === "ru"
                ? "Анализ выполняется строго в режиме чтения (Read-Only). Никакие скрипты и изменения на исходном сервере не выполняются."
                : "Source server inspection is strictly read-only. No scripts or writes are performed."}
            </p>
          </div>

          <div className="flex justify-end pt-2">
            <Button
              size="sm"
              variant="primary"
              disabled={!sourcePath.trim() || inspecting}
              onClick={handleInspect}
            >
              {inspecting && <Loader2 className="h-3.5 w-3.5 mr-1.5 animate-spin" />}
              {locale === "ru" ? "Проанализировать сервер" : "Inspect Server"}
              <ArrowRight className="h-3.5 w-3.5 ml-1.5" />
            </Button>
          </div>
        </Card>
      )}

      {/* STEP 2: INSPECTION REPORT */}
      {step === "inspect" && report && (
        <Card className="p-6 space-y-4">
          <h3 className="text-sm font-semibold text-ink flex items-center gap-2">
            <Server className="h-4 w-4 text-gold" />
            {locale === "ru" ? "Шаг 2: Результаты анализа исходного сервера" : "Step 2: Source Inspection Results"}
          </h3>

          <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
            <div className="rounded-lg border border-line bg-card p-3">
              <span className="text-[11px] text-muted">{locale === "ru" ? "Тип сервера" : "Server Type"}</span>
              <p className="mt-1 font-mono text-xs font-semibold text-ink">
                {report.source_type === "CSM_MANAGED_COA" && "CSM Managed CoA"}
                {report.source_type === "LEGACY_EMBEDDED_COA" && "Legacy CoA (Embedded)"}
                {report.source_type === "STANDARD_AZEROTH_CORE" && "Standard AzerothCore"}
                {report.source_type === "UNKNOWN_CUSTOM" && "Custom Layout"}
              </p>
            </div>
            <div className="rounded-lg border border-line bg-card p-3">
              <span className="text-[11px] text-muted">{locale === "ru" ? "Аккаунтов" : "Accounts"}</span>
              <p className="mt-1 font-mono text-xs font-semibold text-ink flex items-center gap-1.5">
                <Users className="h-3.5 w-3.5 text-gold" />
                {report.accounts_count}
              </p>
            </div>
            <div className="rounded-lg border border-line bg-card p-3">
              <span className="text-[11px] text-muted">{locale === "ru" ? "Персонажей" : "Characters"}</span>
              <p className="mt-1 font-mono text-xs font-semibold text-ink flex items-center gap-1.5">
                <Users className="h-3.5 w-3.5 text-sky-400" />
                {report.characters_count}
              </p>
            </div>
            <div className="rounded-lg border border-line bg-card p-3">
              <span className="text-[11px] text-muted">{locale === "ru" ? "Схема БД" : "DB Schema"}</span>
              <p className="mt-1 font-mono text-xs font-semibold text-ink">
                {report.schema_version ?? "2026_10_28_00"}
              </p>
            </div>
          </div>

          {/* Detected Modules */}
          <div>
            <h4 className="text-xs font-semibold text-ink mb-2">
              {locale === "ru" ? "Обнаруженные модули и таблицы:" : "Detected Modules & Tables:"}
            </h4>
            {report.detected_modules.length === 0 ? (
              <p className="text-xs text-muted italic">
                {locale === "ru" ? "Сторонние модули не обнаружены." : "No external modules detected."}
              </p>
            ) : (
              <div className="divide-y divide-line border border-line rounded-lg">
                {report.detected_modules.map((m) => (
                  <div key={m.id} className="p-3 flex items-center justify-between text-xs">
                    <div>
                      <p className="font-semibold text-ink">{m.name} <span className="font-mono text-[11px] text-muted">({m.id})</span></p>
                      <p className="text-[11px] text-muted">Обнаружен через: {m.detected_via}</p>
                    </div>
                    <span className="rounded bg-ok/10 text-ok border border-ok/30 px-2 py-0.5 text-[11px] font-medium">
                      ✓ Адаптер готов
                    </span>
                  </div>
                ))}
              </div>
            )}
          </div>

          <div className="flex justify-between pt-2">
            <Button size="sm" variant="ghost" onClick={() => setStep("source")}>
              {locale === "ru" ? "Назад" : "Back"}
            </Button>
            <Button size="sm" variant="primary" onClick={() => setStep("scope")}>
              {locale === "ru" ? "Настроить перенос" : "Configure Scope"}
              <ArrowRight className="h-3.5 w-3.5 ml-1.5" />
            </Button>
          </div>
        </Card>
      )}

      {/* STEP 3: SCOPE & CONFLICT RESOLUTION */}
      {step === "scope" && report && (
        <Card className="p-6 space-y-5">
          <h3 className="text-sm font-semibold text-ink flex items-center gap-2">
            <Database className="h-4 w-4 text-gold" />
            {locale === "ru" ? "Шаг 3: Выбор категорий данных и политика конфликтов" : "Step 3: Migration Scope"}
          </h3>

          <div className="space-y-3">
            <label className="flex items-start gap-3 p-3 rounded-lg border border-line bg-card cursor-pointer hover:border-gold/40">
              <input
                type="checkbox"
                checked={scope.accounts_and_characters}
                onChange={(e) => setScope({ ...scope, accounts_and_characters: e.target.checked })}
                className="mt-0.5 h-4 w-4 rounded border-line text-gold focus:ring-gold"
              />
              <div>
                <p className="text-xs font-semibold text-ink">
                  {locale === "ru" ? "Аккаунты и персонажи" : "Accounts & Characters"}
                </p>
                <p className="text-[11px] text-muted">
                  {locale === "ru"
                    ? "Учетные записи, персонажи, инвентарь, таланты, заклинания, символы, навыки, репутация, квесты и питомцы."
                    : "Accounts, characters, inventory, spells, talents, glyphs, skills, reputation, quests, and pets."}
                </p>
              </div>
            </label>

            <label className="flex items-start gap-3 p-3 rounded-lg border border-line bg-card cursor-pointer hover:border-gold/40">
              <input
                type="checkbox"
                checked={scope.module_owned_data}
                onChange={(e) => setScope({ ...scope, module_owned_data: e.target.checked })}
                className="mt-0.5 h-4 w-4 rounded border-line text-gold focus:ring-gold"
              />
              <div>
                <p className="text-xs font-semibold text-ink">
                  {locale === "ru" ? "Данные модулей" : "Module-owned Data"}
                </p>
                <p className="text-[11px] text-muted">
                  {locale === "ru"
                    ? "Сохраненные боты (Companions/Playerbots), данные масштабирования контента, таблицы аукцион-бота и зачарователя."
                    : "Saved bot configurations, content scaling overrides, AH bot data, and NPC enchanter states."}
                </p>
              </div>
            </label>

            <label className="flex items-start gap-3 p-3 rounded-lg border border-line bg-card cursor-pointer hover:border-gold/40">
              <input
                type="checkbox"
                checked={scope.server_configuration}
                onChange={(e) => setScope({ ...scope, server_configuration: e.target.checked })}
                className="mt-0.5 h-4 w-4 rounded border-line text-gold focus:ring-gold"
              />
              <div>
                <p className="text-xs font-semibold text-ink">
                  {locale === "ru" ? "Конфигурация сервера (.conf)" : "Server Configurations (.conf)"}
                </p>
                <p className="text-[11px] text-muted">
                  {locale === "ru"
                    ? "Пользовательские настройки модулей из configs/modules (с сохранением безопасных значений)."
                    : "Preserve customized configs from configs/modules/."}
                </p>
              </div>
            </label>
          </div>

          {/* Conflict Policy */}
          <div className="border-t border-line pt-4 space-y-2">
            <h4 className="text-xs font-semibold text-ink">
              {locale === "ru" ? "Политика разрешения конфликтов GUID:" : "GUID Conflict Resolution Policy:"}
            </h4>
            <div className="grid gap-2 sm:grid-cols-3">
              <button
                type="button"
                onClick={() => setConflictPolicy("remap_colliding_guids")}
                className={cn(
                  "p-3 rounded-lg border text-left text-xs transition-colors",
                  conflictPolicy === "remap_colliding_guids"
                    ? "border-gold bg-gold/10 text-gold"
                    : "border-line bg-card text-muted hover:text-ink"
                )}
              >
                <p className="font-semibold text-ink">Переназначить GUID</p>
                <p className="text-[11px] text-muted mt-1">
                  Безопасное смещение ID персонажей и предметов. Ничего не теряется.
                </p>
              </button>
              <button
                type="button"
                onClick={() => setConflictPolicy("skip_colliding_guids")}
                className={cn(
                  "p-3 rounded-lg border text-left text-xs transition-colors",
                  conflictPolicy === "skip_colliding_guids"
                    ? "border-gold bg-gold/10 text-gold"
                    : "border-line bg-card text-muted hover:text-ink"
                )}
              >
                <p className="font-semibold text-ink">Пропустить дубликаты</p>
                <p className="text-[11px] text-muted mt-1">
                  Персонажи с существующими GUID не импортируются.
                </p>
              </button>
              <button
                type="button"
                onClick={() => setConflictPolicy("overwrite_existing")}
                className={cn(
                  "p-3 rounded-lg border text-left text-xs transition-colors",
                  conflictPolicy === "overwrite_existing"
                    ? "border-bad bg-bad/10 text-bad"
                    : "border-line bg-card text-muted hover:text-ink"
                )}
              >
                <p className="font-semibold text-ink">Перезаписать (Опасно)</p>
                <p className="text-[11px] text-muted mt-1">
                  Заменяет существующих персонажей на целевом сервере.
                </p>
              </button>
            </div>
          </div>

          <div className="flex justify-between pt-2">
            <Button size="sm" variant="ghost" onClick={() => setStep("inspect")}>
              {locale === "ru" ? "Назад" : "Back"}
            </Button>
            <Button size="sm" variant="primary" disabled={planning} onClick={handleCreatePlan}>
              {planning && <Loader2 className="h-3.5 w-3.5 mr-1.5 animate-spin" />}
              {locale === "ru" ? "Сформировать план" : "Generate Plan"}
              <ArrowRight className="h-3.5 w-3.5 ml-1.5" />
            </Button>
          </div>
        </Card>
      )}

      {/* STEP 4: PLAN & RISKS */}
      {step === "plan" && plan && (
        <Card className="p-6 space-y-4">
          <h3 className="text-sm font-semibold text-ink flex items-center gap-2">
            <FileCheck className="h-4 w-4 text-gold" />
            {locale === "ru" ? "Шаг 4: План переноса и подтверждение" : "Step 4: Preflight Plan"}
          </h3>

          <div className="rounded-lg border border-gold/30 bg-gold/5 p-4 text-xs space-y-2">
            <p className="font-semibold text-gold flex items-center gap-1.5">
              <Shield className="h-4 w-4" />
              {locale === "ru" ? "Автоматическое создание точки восстановления" : "Automatic Restore Point"}
            </p>
            <p className="text-muted">
              {locale === "ru"
                ? "Перед началом переноса будет создана полная резервная копия целевой базы данных и файлов. В случае любой ошибки состояние будет полностью восстановлено."
                : "A complete backup of the destination realm will be created before any changes. Full rollback is guaranteed on failure."}
            </p>
          </div>

          {plan.warnings.length > 0 && (
            <div className="space-y-1">
              <h4 className="text-xs font-semibold text-ink flex items-center gap-1.5">
                <AlertTriangle className="h-3.5 w-3.5 text-warn" />
                {locale === "ru" ? "Предупреждения и риски:" : "Warnings:"}
              </h4>
              <ul className="list-disc list-inside text-xs text-muted space-y-1">
                {plan.warnings.map((w, idx) => (
                  <li key={idx}>{w}</li>
                ))}
              </ul>
            </div>
          )}

          <div>
            <h4 className="text-xs font-semibold text-ink mb-1.5">
              {locale === "ru" ? "Таблицы к переносу (" + plan.tables_to_migrate.length + "):" : "Tables to Migrate:"}
            </h4>
            <div className="flex flex-wrap gap-1.5">
              {plan.tables_to_migrate.map((t) => (
                <span key={t} className="rounded bg-white/5 border border-line px-2 py-0.5 text-[11px] font-mono text-muted">
                  {t}
                </span>
              ))}
            </div>
          </div>

          <div className="flex justify-between pt-3 border-t border-line">
            <Button size="sm" variant="ghost" onClick={() => setStep("scope")}>
              {locale === "ru" ? "Назад" : "Back"}
            </Button>
            <Button size="sm" variant="primary" disabled={executing} onClick={handleExecute} className="bg-ok hover:bg-ok/90 text-white">
              {executing && <Loader2 className="h-3.5 w-3.5 mr-1.5 animate-spin" />}
              {locale === "ru" ? "Подтвердить и перенести" : "Start Migration"}
            </Button>
          </div>
        </Card>
      )}

      {/* STEP 5: EXECUTING */}
      {step === "executing" && (
        <Card className="p-8 text-center space-y-4">
          <Loader2 className="h-8 w-8 text-gold animate-spin mx-auto" />
          <h3 className="text-base font-bold text-ink">
            {locale === "ru" ? "Выполняется перенос сервера..." : "Migrating Server Data..."}
          </h3>
          <p className="text-xs text-muted max-w-md mx-auto">
            {locale === "ru"
              ? "Создание резервной копии, перенос персонажей, адаптация таблиц модулей и проверка ссылочной целостности."
              : "Creating backup, remapping character GUIDs, executing module adapters, and verifying integrity."}
          </p>
        </Card>
      )}

      {/* STEP 6: RESULT */}
      {step === "result" && execReport && (
        <Card className="p-6 space-y-5">
          <div className="flex items-center gap-3">
            <CheckCircle2 className="h-8 w-8 text-ok shrink-0" />
            <div>
              <h3 className="text-base font-bold text-ink">
                {locale === "ru" ? "Перенос сервера успешно завершен!" : "Migration Completed Successfully!"}
              </h3>
              <p className="text-xs text-muted">
                {locale === "ru"
                  ? `Все выбранные данные перенесены за ${(execReport.duration_ms / 1000).toFixed(1)} с.`
                  : `All data migrated in ${(execReport.duration_ms / 1000).toFixed(1)}s.`}
              </p>
            </div>
          </div>

          <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
            <div className="rounded-lg border border-line bg-card p-3">
              <span className="text-[11px] text-muted">Аккаунтов</span>
              <p className="font-mono text-sm font-bold text-ink">{execReport.accounts_transferred}</p>
            </div>
            <div className="rounded-lg border border-line bg-card p-3">
              <span className="text-[11px] text-muted">Персонажей</span>
              <p className="font-mono text-sm font-bold text-ink">{execReport.characters_transferred}</p>
            </div>
            <div className="rounded-lg border border-line bg-card p-3">
              <span className="text-[11px] text-muted">Модулей адаптировано</span>
              <p className="font-mono text-sm font-bold text-ok">{execReport.modules_migrated.length}</p>
            </div>
            <div className="rounded-lg border border-line bg-card p-3">
              <span className="text-[11px] text-muted">Таблиц перенесено</span>
              <p className="font-mono text-sm font-bold text-ink">{execReport.tables_migrated.length}</p>
            </div>
          </div>

          {execReport.backup_path && (
            <div className="rounded-lg bg-white/5 border border-line p-3 text-xs text-muted">
              <p className="font-semibold text-ink">{locale === "ru" ? "Точка восстановления:" : "Restore Point:"}</p>
              <p className="font-mono text-[11px] break-all">{execReport.backup_path}</p>
            </div>
          )}

          <div className="flex justify-end pt-2">
            <Button size="sm" variant="primary" onClick={onClose}>
              {locale === "ru" ? "Готово" : "Finish"}
            </Button>
          </div>
        </Card>
      )}
    </div>
  );
}
