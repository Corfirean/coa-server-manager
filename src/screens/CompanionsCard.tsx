import { useEffect, useState } from "react";
import {
  Loader2,
  Bot,
  CheckCircle2,
  AlertCircle,
  RefreshCw,
  Power,
  Users,
  Database,
} from "lucide-react";
import {
  api,
  asUiError,
  type Population,
  type RealmMode,
  type RealmProfiles,
  type UiError,
} from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { useHuman, useT } from "@/i18n";

export function CompanionsCard({ serverId }: { serverId: string }) {
  const t = useT();
  const human = useHuman();

  // Realm & Environment state
  const [profiles, setProfiles] = useState<RealmProfiles | null>(null);
  const [selectedRealm, setSelectedRealm] = useState<RealmMode>("coa");
  const [pop, setPop] = useState<Population | null>(null);
  const [running, setRunning] = useState(false);

  // Fleet Configuration state
  const [spawnMode, setSpawnMode] = useState<"population" | "fleet">("population");
  const [countChoice, setCountChoice] = useState<string>("50");
  const [customCount, setCustomCount] = useState<string>("50");
  const [targetLevel, setTargetLevel] = useState<number>(80);
  const [selectedClass, setSelectedClass] = useState<string>("all");
  const [selectedRole, setSelectedRole] = useState<string>("all");
  const [selectedArchetype, setSelectedArchetype] = useState<string>("all");

  // Operational state
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<UiError | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [job, setJob] = useState<{ baseline: number; target: number; since: number } | null>(null);

  // Startup Auto-Login settings
  const [autoOn, setAutoOn] = useState(true);
  const [autoMax, setAutoMax] = useState("");
  const [startNote, setStartNote] = useState<string | null>(null);
  const [startBusy, setStartBusy] = useState(false);
  const [despawnN, setDespawnN] = useState("10");

  const activeServerRealm = profiles?.active ?? "coa";
  const isViewingActiveRealm = selectedRealm === activeServerRealm;

  const maxLevelForRealm = selectedRealm === "reborn" ? 60 : 80;

  // Load realm profiles
  const loadProfiles = async () => {
    try {
      const r = await api.realmProfiles(serverId);
      setProfiles(r);
      if (!selectedRealm) {
        setSelectedRealm(r.active);
      }
    } catch {
      // ignore
    }
  };

  useEffect(() => {
    void loadProfiles();
  }, [serverId]);

  // Adjust target level if realm changed
  useEffect(() => {
    if (targetLevel > maxLevelForRealm) {
      setTargetLevel(maxLevelForRealm);
    }
  }, [selectedRealm, maxLevelForRealm, targetLevel]);

  // Load startup settings
  useEffect(() => {
    void api
      .settings(serverId, "bots")
      .then((v) => {
        const on = v.settings.find((s) => s.key === "CoaBots.AutoLoginOnStartup");
        const max = v.settings.find((s) => s.key === "CoaBots.AutoLogin.MaxCount");
        if (on) setAutoOn(on.value === true);
        if (max) setAutoMax(String(max.value));
      })
      .catch(() => {});
  }, [serverId]);

  const maxNum = Number(autoMax);
  const maxValid = autoMax.trim() !== "" && Number.isInteger(maxNum) && maxNum >= 0 && maxNum <= 5000;

  // Polling population
  const fetchPopulation = async () => {
    const s = await api.status(serverId).catch(() => null);
    if (!s) return;
    const up = s.observed.world.state === "running";
    setRunning(up);
    if (up) {
      const p = await api.population(serverId, selectedRealm).catch(() => null);
      setPop(p);
    } else {
      setPop(null);
    }
  };

  useEffect(() => {
    let alive = true;
    const poll = async () => {
      if (!alive) return;
      await fetchPopulation();
    };
    void poll();
    const interval = setInterval(poll, job ? 1500 : 4000);
    return () => {
      alive = false;
      clearInterval(interval);
    };
  }, [serverId, selectedRealm, job !== null]); // eslint-disable-line react-hooks/exhaustive-deps

  const done = job && pop ? Math.max(0, Math.min(job.target, pop.bots_total - job.baseline)) : 0;
  useEffect(() => {
    if (!job) return;
    if ((pop && done >= job.target) || !running) {
      if (pop && done >= job.target) setNote(t("comp.progressDone", { n: job.target }));
      setJob(null);
    }
  }, [done, running]); // eslint-disable-line react-hooks/exhaustive-deps

  const count = countChoice === "custom" ? Number(customCount) : Number(countChoice);
  const valid = Number.isInteger(count) && count >= 1 && count <= 2000;

  async function act(fn: () => Promise<string>) {
    setBusy(true);
    setError(null);
    try {
      setNote(await fn());
    } catch (e) {
      setError(asUiError(e));
    } finally {
      setBusy(false);
    }
  }

  // Primary action: Spawn bots / fleet
  const handleSpawn = async () => {
    if (!valid) return;
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      if (spawnMode === "fleet") {
        // Fleet spawn: race 1 (Human) or 2 (Orc)
        const race = 1;
        const out = await api.spawnBotFleet(serverId, race, targetLevel);
        setNote(out || `Spawned level ${targetLevel} fleet`);
      } else {
        const r = await api.addCompanions(serverId, count, selectedRealm);
        if (r.spawned && r.baseline !== null) {
          setJob({ baseline: r.baseline, target: count, since: Date.now() });
        }
        setNote(
          r.created
            ? t("comp.offlineDone", { n: r.created })
            : r.spawned
              ? t("comp.spawned", { n: count })
              : t("comp.saved", { n: count })
        );
      }
      await fetchPopulation();
    } catch (e) {
      setError(asUiError(e));
    } finally {
      setBusy(false);
    }
  };

  // Bring offline bots online
  const handleLoginAll = () =>
    act(async () => {
      const out = await api.loginAllCompanions(serverId);
      await fetchPopulation();
      return out || "All available offline bots queued for login.";
    });

  // Stop queued creations
  const stopCreating = () =>
    act(async () => {
      const r = await api.stopSpawning(serverId);
      setJob(null);
      await fetchPopulation();
      return t("comp.stopped", { n: r.count });
    });

  // Take online bots offline
  const takeOffline = () => {
    if (!window.confirm(t("comp.offlineConfirm"))) return;
    void act(async () => {
      const r = await api.takeOffline(serverId);
      setJob(null);
      await fetchPopulation();
      return t("comp.offlineDone2", { n: r.count });
    });
  };

  // Despawn batch
  const dn = Number(despawnN);
  const dnValid = Number.isInteger(dn) && dn >= 1 && dn <= 5000;
  const despawnSome = () =>
    act(async () => {
      const r = await api.despawnSome(serverId, dn);
      await fetchPopulation();
      return t("comp.despawnSomeDone", { n: r.count });
    });

  // Delete all bots in realm
  const deleteAll = () => {
    if (!window.confirm(t("comp.deleteConfirm", { n: pop?.bots_total ?? 0 }))) return;
    void act(async () => {
      await api.deleteAllCompanions(serverId);
      setJob(null);
      await fetchPopulation();
      return t("comp.deleteDone");
    });
  };

  // Switch server realm
  const switchServerRealm = async (mode: RealmMode) => {
    if (!window.confirm(`Switch server active realm to ${mode.toUpperCase()} and restart?`)) return;
    setBusy(true);
    try {
      await api.realmSelect(serverId, mode, true);
      await loadProfiles();
      setSelectedRealm(mode);
    } catch (e) {
      setError(asUiError(e));
    } finally {
      setBusy(false);
    }
  };

  // Save auto startup settings
  async function saveStart() {
    setStartBusy(true);
    setError(null);
    setStartNote(null);
    try {
      await api.save(serverId, "bots", {
        "CoaBots.AutoLoginOnStartup": autoOn,
        "CoaBots.AutoLogin.MaxCount": maxNum,
      });
      setStartNote(t("comp.startSaved"));
    } catch (e) {
      setError(asUiError(e));
    } finally {
      setStartBusy(false);
    }
  }

  // Database schema name for selected realm
  const dbSchema =
    selectedRealm === "coa"
      ? "acore_characters"
      : selectedRealm === "wildcard"
        ? "acore_characters_wildcard"
        : "acore_characters_reborn";

  // Binary compatibility check
  const isBinaryCompatible =
    selectedRealm === "coa" ||
    (selectedRealm === "wildcard" && (profiles?.supported ?? true)) ||
    (selectedRealm === "reborn" && (profiles?.reborn_supported ?? true));

  return (
    <Card className="mt-6 p-6 space-y-6">
      {/* Header & Realm Switcher */}
      <div>
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="flex items-center gap-2">
            <Bot className="h-6 w-6 text-gold" />
            <h2 className="text-xl font-bold tracking-tight">{t("comp.title")}</h2>
          </div>
          <div className="flex items-center gap-2">
            <span className="text-xs text-muted flex items-center gap-1">
              <Database className="h-3.5 w-3.5" />
              DB: <code className="text-gold">{dbSchema}</code>
            </span>
          </div>
        </div>

        {/* Realm Navigation Tabs */}
        <div className="mt-4 flex flex-wrap gap-2 border-b border-line pb-3">
          {(["coa", "wildcard", "reborn"] as RealmMode[]).map((mode) => {
            const isSelected = selectedRealm === mode;
            const isActive = activeServerRealm === mode;
            const label =
              mode === "coa"
                ? "Conquest of Azeroth"
                : mode === "wildcard"
                  ? "Wildcard (Hero)"
                  : "Warcraft Reborn";
            return (
              <button
                key={mode}
                type="button"
                onClick={() => setSelectedRealm(mode)}
                className={`flex items-center gap-2 rounded-lg px-4 py-2 text-sm font-medium transition-colors ${
                  isSelected
                    ? "bg-gold/20 text-gold border border-gold/40 shadow-sm"
                    : "border border-line hover:bg-white/5 text-muted hover:text-white"
                }`}
              >
                <span>{label}</span>
                {isActive && (
                  <span className="rounded bg-ok/20 px-1.5 py-0.5 text-[10px] font-semibold text-ok uppercase">
                    Active Server
                  </span>
                )}
              </button>
            );
          })}
        </div>
      </div>

      {/* Binary Compatibility & Realm Mode Warning */}
      {!isBinaryCompatible && (
        <div className="rounded-lg border border-bad/40 bg-bad/10 p-4 text-sm text-bad flex items-start gap-3">
          <AlertCircle className="h-5 w-5 shrink-0 mt-0.5" />
          <div>
            <div className="font-semibold">Binary Compatibility Warning</div>
            <p className="mt-1 text-xs text-bad/90">
              The currently running server core binary does not have full multi-realm support enabled
              for <strong>{selectedRealm.toUpperCase()}</strong>. Automated bot features may be restricted.
            </p>
          </div>
        </div>
      )}

      {/* Active Realm Switch Helper */}
      {!isViewingActiveRealm && (
        <div className="rounded-lg border border-warn/40 bg-warn/10 p-3 text-sm flex items-center justify-between gap-3">
          <span className="text-warn text-xs">
            Viewing <strong>{selectedRealm.toUpperCase()}</strong> bots, but the worldserver is currently running in{" "}
            <strong>{activeServerRealm.toUpperCase()}</strong> mode.
          </span>
          <Button
            size="sm"
            variant="secondary"
            disabled={busy}
            onClick={() => void switchServerRealm(selectedRealm)}
          >
            Switch Server to {selectedRealm.toUpperCase()}
          </Button>
        </div>
      )}

      {/* Realm Population Dashboard */}
      <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
        <div className="rounded-lg border border-line bg-bg/50 p-3">
          <div className="text-xs text-muted flex items-center gap-1.5">
            <Bot className="h-3.5 w-3.5 text-gold" />
            Bots Online
          </div>
          <div className="mt-1 text-2xl font-bold text-white">
            {running && pop ? pop.bots_online : "—"}
          </div>
        </div>
        <div className="rounded-lg border border-line bg-bg/50 p-3">
          <div className="text-xs text-muted flex items-center gap-1.5">
            <Users className="h-3.5 w-3.5 text-gold" />
            Total Fleet
          </div>
          <div className="mt-1 text-2xl font-bold text-white">
            {pop ? pop.bots_total : "—"}
          </div>
        </div>
        <div className="rounded-lg border border-line bg-bg/50 p-3">
          <div className="text-xs text-muted flex items-center gap-1.5">
            <CheckCircle2 className="h-3.5 w-3.5 text-ok" />
            Players Online
          </div>
          <div className="mt-1 text-2xl font-bold text-white">
            {running && pop ? pop.players_online : "—"}
          </div>
        </div>
        <div className="rounded-lg border border-line bg-bg/50 p-3 flex flex-col justify-between">
          <div className="text-xs text-muted flex items-center justify-between">
            <span>Status</span>
            <button
              onClick={() => void fetchPopulation()}
              title="Refresh"
              className="text-muted hover:text-white"
            >
              <RefreshCw className="h-3 w-3" />
            </button>
          </div>
          <div className="text-sm font-medium">
            {running ? (
              <span className="text-ok flex items-center gap-1">
                <span className="h-2 w-2 rounded-full bg-ok animate-pulse" /> Running
              </span>
            ) : (
              <span className="text-muted">Offline</span>
            )}
          </div>
        </div>
      </div>

      {/* Fleet Configuration & Spawning Section */}
      <div className="border-t border-line pt-5 space-y-4">
        <div className="flex items-center justify-between">
          <h3 className="font-semibold text-sm tracking-wide uppercase text-muted">
            Fleet Deployment Controls
          </h3>
          <div className="flex rounded-md border border-line p-0.5 text-xs">
            <button
              type="button"
              onClick={() => setSpawnMode("population")}
              className={`px-2.5 py-1 rounded transition-colors ${
                spawnMode === "population" ? "bg-gold/20 text-gold font-medium" : "text-muted hover:text-white"
              }`}
            >
              Batch Population
            </button>
            <button
              type="button"
              onClick={() => setSpawnMode("fleet")}
              className={`px-2.5 py-1 rounded transition-colors ${
                spawnMode === "fleet" ? "bg-gold/20 text-gold font-medium" : "text-muted hover:text-white"
              }`}
            >
              Synchronous Fleet
            </button>
          </div>
        </div>

        {/* Count Selector */}
        {spawnMode === "population" && (
          <div>
            <label className="text-xs text-muted">Fleet Size (Number of Bots)</label>
            <div className="mt-2 flex flex-wrap gap-2">
              {["10", "25", "50", "100", "250", "500"].map((n) => (
                <button
                  key={n}
                  type="button"
                  onClick={() => setCountChoice(n)}
                  className={`rounded-md border px-3 py-1.5 text-sm transition-colors ${
                    countChoice === n
                      ? "border-gold bg-gold/15 text-gold font-medium"
                      : "border-line hover:bg-white/5 text-muted"
                  }`}
                >
                  {n}
                </button>
              ))}
              <button
                type="button"
                onClick={() => setCountChoice("custom")}
                className={`rounded-md border px-3 py-1.5 text-sm transition-colors ${
                  countChoice === "custom"
                    ? "border-gold bg-gold/15 text-gold font-medium"
                    : "border-line hover:bg-white/5 text-muted"
                }`}
              >
                Custom
              </button>
              {countChoice === "custom" && (
                <input
                  type="number"
                  min="1"
                  max="2000"
                  value={customCount}
                  onChange={(e) => setCustomCount(e.target.value)}
                  className="w-24 rounded-md border border-line bg-bg px-3 py-1.5 text-right text-sm outline-none focus:border-gold"
                />
              )}
            </div>
          </div>
        )}

        {/* Fleet Specific Settings: Level, Class, Role, Archetype */}
        <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-3">
          {/* Target Level */}
          <div>
            <label className="text-xs text-muted">
              {spawnMode === "fleet" ? "Fleet Level" : "Max Level Cap"} (1 - {maxLevelForRealm})
            </label>
            <input
              type="number"
              min="1"
              max={maxLevelForRealm}
              value={targetLevel}
              onChange={(e) => setTargetLevel(Math.max(1, Math.min(maxLevelForRealm, Number(e.target.value))))}
              className="mt-1.5 w-full rounded-md border border-line bg-bg px-3 py-1.5 text-sm outline-none focus:border-gold"
            />
          </div>

          {/* Class Filter */}
          <div>
            <label className="text-xs text-muted">Class Focus</label>
            <select
              value={selectedClass}
              onChange={(e) => setSelectedClass(e.target.value)}
              className="mt-1.5 w-full rounded-md border border-line bg-bg px-3 py-1.5 text-sm outline-none focus:border-gold"
            >
              <option value="all">All Realm Classes</option>
              {selectedRealm === "coa" && (
                <>
                  <option value="12">Guardian</option>
                  <option value="13">Starcaller</option>
                  <option value="14">Demon Hunter</option>
                  <option value="15">Necromancer</option>
                  <option value="16">Monk</option>
                  <option value="17">Pyromancer</option>
                  <option value="18">Chronomancer</option>
                  <option value="19">Cultist</option>
                  <option value="20">Runemaster</option>
                  <option value="21">Primalist</option>
                  <option value="22">Tinker</option>
                </>
              )}
              {selectedRealm === "wildcard" && (
                <option value="10">Hero (Freepick)</option>
              )}
              {selectedRealm === "reborn" && (
                <>
                  <option value="1">Warrior</option>
                  <option value="2">Paladin</option>
                  <option value="3">Hunter</option>
                  <option value="4">Rogue</option>
                  <option value="5">Priest</option>
                  <option value="6">Death Knight</option>
                  <option value="7">Shaman</option>
                  <option value="8">Mage</option>
                  <option value="9">Warlock</option>
                  <option value="11">Druid</option>
                </>
              )}
            </select>
          </div>

          {/* Role Filter */}
          <div>
            <label className="text-xs text-muted">Combat Role</label>
            <select
              value={selectedRole}
              onChange={(e) => setSelectedRole(e.target.value)}
              className="mt-1.5 w-full rounded-md border border-line bg-bg px-3 py-1.5 text-sm outline-none focus:border-gold"
            >
              <option value="all">All Roles</option>
              <option value="tank">Tank</option>
              <option value="healer">Healer</option>
              <option value="melee">Melee DPS</option>
              <option value="ranged">Ranged Physical DPS</option>
              <option value="caster">Caster DPS</option>
            </select>
          </div>

          {/* Archetype Filter */}
          <div>
            <label className="text-xs text-muted">Archetype / Build</label>
            <select
              value={selectedArchetype}
              onChange={(e) => setSelectedArchetype(e.target.value)}
              className="mt-1.5 w-full rounded-md border border-line bg-bg px-3 py-1.5 text-sm outline-none focus:border-gold"
            >
              <option value="all">Auto / Integral Archetype</option>
              {selectedRealm === "wildcard" && (
                <>
                  <option value="bear">Bear Tank (Guardian)</option>
                  <option value="murlock">Mur-Lock (Unholy/Frost)</option>
                  <option value="firesage">Fire Sage (Fire/Ele)</option>
                  <option value="forestwarden">Forest Warden (Nature/Feral)</option>
                  <option value="bleedrogue">Bleed Rogue / Berserker</option>
                  <option value="holypal">Holy Paladin / Healer</option>
                </>
              )}
              {selectedRealm === "reborn" && (
                <>
                  <option value="spiritguard">Spirit Guard Shaman (Tank)</option>
                  <option value="darkapoth">Dark Apotheosis Warlock (Tank)</option>
                  <option value="guardianbear">Guardian Bear Druid (Tank)</option>
                  <option value="protpal">Protection Paladin (Tank)</option>
                  <option value="protwar">Protection Warrior (Tank)</option>
                  <option value="holypriest">Holy Priest (Healer)</option>
                  <option value="restodruid">Restoration Druid / Shaman</option>
                </>
              )}
              {selectedRealm === "coa" && (
                <option value="class_spec">Class Mastery Specialization</option>
              )}
            </select>
          </div>
        </div>

        {/* Deployment Actions */}
        <div className="flex flex-wrap items-center gap-3 pt-2">
          <Button
            variant="primary"
            disabled={busy || !valid || job !== null || !isViewingActiveRealm}
            onClick={() => void handleSpawn()}
          >
            {busy && <Loader2 className="h-4 w-4 animate-spin mr-2" />}
            {spawnMode === "fleet"
              ? `Spawn Level ${targetLevel} Fleet`
              : `Deploy ${valid ? count : ""} Bots`}
          </Button>

          {running && pop && pop.bots_total > pop.bots_online && (
            <Button
              variant="secondary"
              disabled={busy || job !== null || !isViewingActiveRealm}
              onClick={() => void handleLoginAll()}
              title="Bring all created bots online"
            >
              <Power className="h-4 w-4 mr-1.5 text-ok" />
              Bring All Online ({pop.bots_total - pop.bots_online} idle)
            </Button>
          )}

          {!running && (
            <span className="text-xs text-muted">
              Server stopped: bots will be created directly in DB or queued for startup.
            </span>
          )}
        </div>

        {/* Progress Bar during active spawn */}
        {job && (
          <div className="rounded-lg border border-line bg-bg/40 p-4 space-y-2">
            <div className="flex justify-between text-sm">
              <span className="font-medium">
                Creating bots: {done} / {job.target}
              </span>
              <span className="text-gold font-bold">
                {Math.round((done / job.target) * 100)}%
              </span>
            </div>
            <div className="h-2 overflow-hidden rounded-full bg-white/10">
              <div
                className="h-full rounded-full bg-gold transition-[width] duration-300"
                style={{ width: `${(done / job.target) * 100}%` }}
              />
            </div>
            <div className="flex justify-between items-center pt-1">
              <span className="text-xs text-muted">
                Paced creation prevents server stalls and guarantees full talent/gear setup.
              </span>
              <Button size="sm" variant="ghost" disabled={busy} onClick={() => void stopCreating()}>
                Cancel Queue
              </Button>
            </div>
          </div>
        )}

        {note && !job && (
          <p className="text-sm text-ok flex items-center gap-1.5">
            <CheckCircle2 className="h-4 w-4 shrink-0" />
            {note}
          </p>
        )}
      </div>

      {/* Online Fleet Management (Despawn, Purge) */}
      {running && pop && pop.bots_total > 0 && isViewingActiveRealm && (
        <div className="border-t border-line pt-5 space-y-3">
          <h3 className="font-semibold text-sm tracking-wide uppercase text-muted">
            Online Fleet Management
          </h3>

          <div className="flex flex-wrap items-center gap-3 text-sm">
            <label htmlFor="despawn-n" className="text-muted text-xs">
              Despawn Batch:
            </label>
            <input
              id="despawn-n"
              value={despawnN}
              onChange={(e) => setDespawnN(e.target.value)}
              inputMode="numeric"
              aria-invalid={!dnValid}
              className="w-20 rounded-md border border-line bg-bg px-2.5 py-1 text-right text-sm outline-none focus:border-gold"
            />
            <Button
              size="sm"
              variant="secondary"
              disabled={busy || !dnValid}
              onClick={() => void despawnSome()}
            >
              Despawn {dnValid ? dn : "…"} Bots
            </Button>

            <Button size="sm" variant="secondary" disabled={busy} onClick={takeOffline}>
              Take All Offline
            </Button>

            <Button
              size="sm"
              variant="ghost"
              className="text-bad hover:text-bad ml-auto"
              disabled={busy}
              onClick={deleteAll}
            >
              Purge Entire Fleet ({pop.bots_total})
            </Button>
          </div>
        </div>
      )}

      {/* Startup Auto-Login Configuration */}
      <div className="border-t border-line pt-5 space-y-3">
        <h3 className="font-semibold text-sm tracking-wide uppercase text-muted">
          Persistent Startup Fleet
        </h3>
        <p className="text-xs text-muted">
          Configure how many bots automatically log into the world whenever the server launches.
        </p>

        <div className="flex flex-wrap items-center gap-4 text-sm">
          <label className="flex cursor-pointer items-center gap-2">
            <input
              type="checkbox"
              checked={autoOn}
              onChange={(e) => setAutoOn(e.target.checked)}
              className="rounded border-line text-gold focus:ring-gold"
            />
            <span>Enable Auto-Login on Server Startup</span>
          </label>

          <div className="flex items-center gap-2">
            <label htmlFor="start-max" className="text-xs text-muted">
              Startup Limit:
            </label>
            <input
              id="start-max"
              value={autoMax}
              onChange={(e) => setAutoMax(e.target.value)}
              inputMode="numeric"
              disabled={!autoOn}
              aria-invalid={!maxValid}
              className="w-24 rounded-md border border-line bg-bg px-2.5 py-1 text-right text-sm outline-none focus:border-gold disabled:opacity-40"
            />
            <Button
              size="sm"
              variant="secondary"
              disabled={startBusy || (autoOn && !maxValid)}
              onClick={() => void saveStart()}
            >
              Save Setting
            </Button>
          </div>
        </div>
        {startNote && <p className="text-xs text-ok">{startNote}</p>}
      </div>

      {error && (
        <div className="rounded-md border border-bad/40 bg-bad/10 p-3 text-sm text-bad flex items-center gap-2">
          <AlertCircle className="h-4 w-4 shrink-0" />
          <span>{error.human.code === "unknown" ? error.technical : human(error.human).message}</span>
        </div>
      )}
    </Card>
  );
}
