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

export interface Preflight {
  ok: boolean;
  problems: { code: string; message: string }[];
  free_bytes: number;
}

export interface InstallStep {
  step: string;
  percent: number;
  detail: string | null;
}

export interface UpdatePlanItem {
  path: string;
  action: "create" | "replace" | "merge-config" | "skip" | "conflict";
  reason: string | null;
}

export interface UpdatePreview {
  from_version: string | null;
  to_version: string;
  items: UpdatePlanItem[];
  conflicts: string[];
  migrations: number;
  download_bytes: number;
}

export interface UpdateTxn {
  id: string;
  state: "prepared" | "applying" | "applied" | "needs-decision" | "committed" | "rolled-back" | "failed";
  from_version: string | null;
  to_version: string;
  recovery_point: string | null;
  message: string | null;
}

export interface Population {
  online_total: number;
  bots_online: number;
  players_online: number;
  bots_total: number;
}

export interface Performance {
  mean_ms: number;
  median_ms: number;
  p95_ms: number;
  p99_ms: number;
  max_ms: number;
  ticks_per_sec: number;
}

export interface CompanionSizes {
  hardware: { cores: number; ram_gb: number; free_ram_gb: number };
  sizes: { id: string; title: string; bots: number; warning: string | null }[];
}

export interface ClientInfo {
  path: string;
  executable: string;
  realmlists: { path: string; host: string | null }[];
  addon: { installed: boolean; version: string | null; up_to_date: boolean | null };
  other_addons: number;
}

export type FriendsMode = "local" | "lan" | "direct" | "private";

export interface FriendsStatus {
  settings: { mode: FriendsMode; host: string | null };
  lan_ip: string | null;
  exposure: { port: number; what: string; reachable_from_network: boolean; listening: boolean }[];
  servers_open: boolean;
  firewall: { auth: boolean; world: boolean };
  tailscale: { installed: boolean; ip: string | null; connected: boolean };
  server_running: boolean;
  auth_port: number;
  world_port: number;
}

export interface InternetCheck {
  public_ip: string | null;
  router_ip: string | null;
  reachability: "direct_possible" | "cgnat" | "unknown";
  router_found: boolean;
}

export interface DiagCheck {
  id: string;
  title: string;
  level: "ok" | "warn" | "fail";
  detail: string;
}

export type ConsoleSource = "world" | "auth" | "database" | "manager";
export interface ConsoleLine {
  text: string;
  level: "info" | "warn" | "error";
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
  installPreflight: (dest: string) => invoke<Preflight>("install_preflight", { dest }),
  installNew: (dest: string, pkg?: string) => invoke<ServerSummary>("install_new", { dest, package: pkg ?? null }),
  cancelInstall: () => invoke<void>("cancel_install"),
  createAccount: (id: string, username: string, password: string, administrator: boolean) =>
    invoke<void>("create_account", { id, username, password, administrator }),
  checkUpdate: (id: string, source?: string) => invoke<UpdatePreview>("check_update", { id, source: source ?? null }),
  pendingUpdate: (id: string) => invoke<UpdateTxn | null>("pending_update", { id }),
  applyUpdate: (id: string, resolutions: Record<string, "keep" | "replace">, source?: string) =>
    invoke<{ txn: UpdateTxn }>("apply_update", { id, source: source ?? null, resolutions }),
  rollbackUpdate: (id: string, txn: string) => invoke<UpdateTxn>("rollback_update", { id, txn }),
  population: (id: string) => invoke<Population | null>("get_population", { id }),
  openLink: (url: string) => invoke<void>("open_link", { url }),
  performance: (id: string) => invoke<Performance | null>("get_performance", { id }),
  companionSizes: () => invoke<CompanionSizes>("companion_sizes"),
  addCompanions: (id: string, count: number) => invoke<{ spawned: string | null; created: number | null }>("add_companions", { id, count }),
  clientInfo: (id: string) => invoke<ClientInfo | null>("client_info", { id }),
  setClient: (id: string, path: string) => invoke<ClientInfo>("set_client", { id, path }),
  setRealmlist: (id: string, host: string) => invoke<string[]>("client_realmlist", { id, host }),
  installAddon: (id: string) => invoke<void>("client_install_addon", { id }),
  play: (id: string) => invoke<DriverOutcome>("play", { id }),
  friendsStatus: (id: string) => invoke<FriendsStatus>("friends_status", { id }),
  friendsCheckInternet: () => invoke<InternetCheck>("friends_check_internet"),
  friendsEnable: (id: string, mode: FriendsMode, host?: string, useUpnp = false) =>
    invoke<{ host: string; restart_required: boolean; note: string | null }>("friends_enable", { id, mode, host: host ?? null, useUpnp }),
  friendsPackage: (id: string) => invoke<string>("friends_package", { id }),
  runDiagnostics: (id: string) => invoke<{ checks: DiagCheck[]; problems: number }>("run_diagnostics", { id }),
  verifyFiles: (id: string) => invoke<{ path: string; kind: "missing" | "changed" }[]>("verify_files", { id }),
  exportDiagnostics: (id: string) => invoke<string>("export_diagnostics", { id }),
  consoleTail: (id: string, source: ConsoleSource, filter?: string, lines = 300) =>
    invoke<ConsoleLine[]>("console_tail", { id, source, filter: filter ?? null, lines }),
  consoleRisk: (command: string) => invoke<"normal" | "dangerous">("console_risk", { command }),
  consoleCommand: (id: string, command: string, confirmed: boolean) => invoke<string>("console_command", { id, command, confirmed }),
  previewPreset: (id: string, scope: Scope, preset: string) => invoke<PresetPreview>("preview_preset", { id, scope, preset }),
};

export function asUiError(e: unknown): UiError {
  if (e && typeof e === "object" && "human" in e) return e as UiError;
  return {
    human: { code: "unknown", title: "Something went wrong", message: "See the technical details.", actions: ["show_details"] },
    technical: String(e),
  };
}
