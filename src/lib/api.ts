import { invoke } from "@tauri-apps/api/core";

export type Classification = "healthy" | "partial" | "unknown-custom" | "incompatible";
export type ItemStatus = "found" | "missing" | "attention";

export interface ScanItem {
  key: string;
  label: string;
  status: ItemStatus;
  detail: string | null;
}

export interface ExeInfo {
  size: number;
  sha256: string;
  matches_release: boolean | null;
}

export interface ScanReport {
  path: string;
  classification: Classification;
  items: ScanItem[];
  worldserver: ExeInfo | null;
  authserver: ExeInfo | null;
  banner_revision: string | null;
  bot_config_keys: number;
  client: { path: string; executable: string } | null;
  notes: string[];
  modifies_files: boolean;
}

export type ServiceState = "stopped" | "starting" | "running" | "stopping" | "crashed" | "updating" | "unknown";

export interface ServiceStatus {
  name: string;
  state: ServiceState;
  pid: number | null;
  port: number;
  port_ready: boolean;
  conflict: { port: number; pid: number; exe: string | null } | null;
  uptime_secs: number | null;
}

export interface StatusView {
  observed: { mysql: ServiceStatus; auth: ServiceStatus; world: ServiceStatus };
  busy: boolean;
  path_exists: boolean;
}

export interface Human {
  code: string;
  title: string;
  message: string;
  actions: string[];
}

export interface DriverOutcome {
  ok: boolean;
  exit_code: number | null;
  code: string | null;
  human: Human | null;
  output: string;
}

export interface ServerSummary {
  id: string;
  name: string;
  path: string;
}

export interface FieldError {
  key: string;
  message: string;
}

export interface UiError {
  human: Human;
  technical: string;
  fields?: FieldError[];
}

export type Scope = "bots" | "server";
export type Restart = "runtime" | "world" | "full";
export type JsonValue = string | number | boolean;

export interface Setting {
  key: string;
  type: "bool" | "int" | "float" | "string" | "enum";
  category: string;
  title: string;
  description: string;
  default: JsonValue;
  min?: number;
  max?: number;
  options: { value: JsonValue; label: string }[];
  unit?: string | null;
  advanced: boolean;
  restartRequired: Restart;
  dangerous: boolean;
}

export interface SettingView extends Setting {
  value: JsonValue;
  is_default: boolean;
  present: boolean;
  problem: string | null;
  drift: boolean;
}

export interface SettingsView {
  scope: Scope;
  categories: { id: string; title: string }[];
  settings: SettingView[];
  unknown_keys: number;
  drift_keys: string[];
  files: string[];
}

export interface PresetInfo {
  id: string;
  title: string;
  description: string;
}

export interface PresetChange {
  key: string;
  title: string;
  from: JsonValue;
  to: JsonValue;
  dangerous: boolean;
}

export interface PresetPreview {
  id: string;
  title: string;
  description: string;
  changes: PresetChange[];
}

export interface SaveReport {
  changed: { key: string; title: string; restart: Restart; dangerous: boolean }[];
  restart: Restart | null;
  snapshot: string | null;
}

export type BackupKind = "quick" | "full" | "config" | "database";

export interface RecoveryPoint {
  schema: number;
  id: string;
  kind: BackupKind;
  trigger: string;
  label: string | null;
  created_at: string;
  components: { name: string; path: string; bytes: number; tables: number | null; files: string[] | null }[];
}

export interface DbRestore {
  previous_schema: string;
  safety_backup: string;
  tables_restored: number;
}

export const api = {
  defaultInstallDir: () => invoke<string>("default_install_dir"),
  scan: (path: string) => invoke<ScanReport>("scan_server", { path }),
  add: (path: string) => invoke<ServerSummary>("add_server", { path }),
  list: () => invoke<ServerSummary[]>("list_servers"),
  forget: (id: string) => invoke<void>("forget_server", { id }),
  status: (id: string) => invoke<StatusView>("server_status", { id }),
  start: (id: string) => invoke<DriverOutcome>("start_server", { id }),
  stop: (id: string) => invoke<DriverOutcome>("stop_server", { id }),
  settings: (id: string, scope: Scope) => invoke<SettingsView>("get_settings", { id, scope }),
  save: (id: string, scope: Scope, changes: Record<string, JsonValue>) => invoke<SaveReport>("save_settings", { id, scope, changes }),
  presets: (scope: Scope) => invoke<PresetInfo[]>("list_presets", { scope }),
  backups: (id: string) => invoke<RecoveryPoint[]>("list_backups", { id }),
  createBackup: (id: string, kind: BackupKind, label?: string) => invoke<RecoveryPoint>("create_backup", { id, kind, label: label ?? null }),
  verifyBackup: (id: string, backupId: string) => invoke<{ ok: boolean; problems: string[] }>("verify_backup", { id, backupId }),
  deleteBackup: (id: string, backupId: string) => invoke<void>("delete_backup", { id, backupId }),
  restoreConfigs: (id: string, backupId: string) => invoke<RecoveryPoint>("restore_backup_configs", { id, backupId }),
  restoreDatabase: (id: string, backupId: string, database: string) => invoke<DbRestore>("restore_backup_database", { id, backupId, database }),
  previewPreset: (id: string, scope: Scope, preset: string) => invoke<PresetPreview>("preview_preset", { id, scope, preset }),
};

export function asUiError(e: unknown): UiError {
  if (e && typeof e === "object" && "human" in e) return e as UiError;
  return {
    human: { code: "unknown", title: "Something went wrong", message: "See the technical details.", actions: ["show_details"] },
    technical: String(e),
  };
}
