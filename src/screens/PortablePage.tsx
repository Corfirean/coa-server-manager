import { useEffect, useMemo, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { AlertTriangle, CheckCircle2, History, Loader2, Plus, ShieldAlert, Swords, Trash2, X } from "lucide-react";
import { useT, type Key } from "@/i18n";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { cn } from "@/lib/utils";
import {
  asPortableError, portable, usePortable,
  type CharacterView, type HistoryEntry, type LocalCharacterView, type Note, type PlayStatus,
  type PlayView, type PortableError, type PreflightView, type RealmView,
} from "@/lib/portable";

const WORKING: PlayStatus[] = ["preparing", "syncing", "saving"];
const LIVE: PlayStatus[] = ["playing", "waiting_login", "preparing", "syncing", "saving"];

function when(iso: string): string {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
}

function StatusChip({ status, degraded }: { status: PlayStatus; degraded?: boolean }) {
  const t = useT();
  const tone = status === "playing" ? "bg-ok/15 text-ok"
    : status === "conflict" || status === "incompatible" || status === "error" ? "bg-bad/15 text-bad"
    : status === "update_required" || status === "compat_warning" || status === "offline" ? "bg-warn/15 text-warn"
    : WORKING.includes(status) || status === "waiting_login" ? "bg-gold/15 text-gold"
    : "bg-white/5 text-muted";
  return <span className="inline-flex items-center gap-2">
    <span className={cn("inline-flex items-center gap-1.5 rounded-full px-2.5 py-0.5 text-xs font-medium", tone)} data-status={status}>
      {WORKING.includes(status) && <Loader2 className="h-3 w-3 animate-spin" aria-hidden />}
      {t(`portable.status.${status}` as Key)}
    </span>
    {degraded && <span className="inline-flex items-center gap-1 rounded-full bg-warn/15 px-2.5 py-0.5 text-xs font-medium text-warn"><AlertTriangle className="h-3 w-3" aria-hidden />{t("portable.status.compat_warning")}</span>}
  </span>;
}

/** The words of one line of the compatibility check. */
function noteText(t: ReturnType<typeof useT>, n: Note): string {
  const key = `portable.note.${n.code}` as Key;
  const text = t(key, { ...n.params });
  return text === key ? t("portable.note.generic", { detail: n.detail }) : text;
}

export function Notes({ notes }: { notes: Note[] }) {
  const t = useT();
  const shown = notes.filter((n) => n.code !== "projection");
  if (!shown.length) return null;
  return <ul className="mt-3 space-y-1.5 text-sm text-muted">{shown.map((n, i) => <li key={i} className="flex gap-2"><AlertTriangle className="mt-0.5 h-4 w-4 shrink-0 text-warn" aria-hidden /><span className="selectable">{noteText(t, n)}</span></li>)}</ul>;
}

export function Projection({ from, to, cap }: { from: number; to: number; cap?: number | null }) {
  const t = useT();
  return <div className="mt-3 rounded-md border border-gold/40 bg-gold/10 p-3 text-sm" role="note">
    <p className="font-medium text-gold">{t("portable.projection.title", { cap: cap ?? to })}</p>
    <p className="mt-1">{t("portable.projection.text", { from, to })}</p>
    <p className="mt-1 text-muted">{t("portable.projection.safe", { from })}</p>
  </div>;
}

function ErrorLine({ error }: { error: PortableError }) {
  const t = useT();
  const key = `portable.err.${error.code}` as Key;
  const text = t(key);
  return <div className="mt-3 text-sm text-bad" role="alert">
    <p>{text === key ? t("portable.err.other") : text}</p>
    <details className="mt-1 text-xs text-muted"><summary className="cursor-pointer">{t("portable.details")}</summary><p className="selectable mt-1">{error.message}</p></details>
    <Notes notes={error.notes} />
  </div>;
}

// ---- Play --------------------------------------------------------------------------------------------------------------------

function PlayDialog({ character, realms, initial, onClose, onChanged }: { character: CharacterView; realms: RealmView[]; initial: string; onClose: () => void; onChanged: () => void }) {
  const t = useT();
  const [realmId, setRealmId] = useState(initial);
  const realm = realms.find((r) => r.id === realmId)!;
  const [pre, setPre] = useState<PreflightView | null>(null);
  const [account, setAccount] = useState(realm.game_account ?? "");
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState<PlayView | null>(null);
  const [error, setError] = useState<PortableError | null>(null);
  const [launching, setLaunching] = useState(false);

  useEffect(() => {
    let alive = true;
    setPre(null); setError(null); setDone(null); setAccount(realm.game_account ?? "");
    void portable.preflight(character.id, realmId).then((p) => { if (alive) setPre(p); }).catch((e) => { if (alive) setError(asPortableError(e)); });
    return () => { alive = false; };
  }, [character.id, realmId]); // eslint-disable-line react-hooks/exhaustive-deps

  async function play() {
    setBusy(true); setError(null);
    try { setDone(await portable.play(character.id, realmId, account.trim() || undefined)); onChanged(); }
    catch (e) { setError(asPortableError(e)); }
    finally { setBusy(false); }
  }
  async function launch() {
    setLaunching(true); setError(null);
    try { await portable.launch(realmId); } catch (e) { setError(asPortableError(e)); } finally { setLaunching(false); }
  }
  async function resolve(action: "use_canonical" | "detach") {
    setBusy(true); setError(null);
    try { await portable.resolve(character.id, realmId, action); onChanged(); setPre(await portable.preflight(character.id, realmId)); }
    catch (e) { setError(asPortableError(e)); }
    finally { setBusy(false); }
  }

  const blocked = !pre || pre.step === "blocked" || pre.step === "offline" || pre.step === "resolve" || busy;
  return <div className="fixed inset-0 z-40 flex items-center justify-center bg-black/60 p-6" role="dialog" aria-modal="true" aria-labelledby="play-title">
    <Card className="max-h-full w-full max-w-xl overflow-y-auto p-6">
      <div className="flex items-start justify-between">
        <h2 id="play-title" className="text-lg font-semibold">{t("portable.play.title", { name: character.name })}</h2>
        <button onClick={onClose} aria-label={t("portable.cancel")} className="cursor-pointer text-muted hover:text-ink"><X className="h-5 w-5" /></button>
      </div>
      <label className="mt-4 block text-sm font-medium" htmlFor="play-realm">{t("portable.play.realm")}</label>
      <select id="play-realm" value={realmId} onChange={(e) => setRealmId(e.target.value)} disabled={busy || !!done} className="mt-1 w-full rounded-md border border-line bg-card px-3 py-2">
        {realms.map((r) => <option key={r.id} value={r.id}>{r.name}{r.level_cap ? ` · ${t("portable.realm.cap", { cap: r.level_cap })}` : ""}</option>)}
      </select>

      {!pre && !error && <p className="mt-4 flex items-center gap-2 text-sm text-muted"><Loader2 className="h-4 w-4 animate-spin" aria-hidden />{t("portable.play.checking")}</p>}
      {pre && !done && <>
        <p className={cn("mt-4 flex items-center gap-2 text-sm font-medium", pre.verdict === "compatible" ? "text-ok" : pre.verdict === "degraded" ? "text-warn" : "text-bad")}>
          {pre.verdict === "compatible" ? <CheckCircle2 className="h-4 w-4" aria-hidden /> : <ShieldAlert className="h-4 w-4" aria-hidden />}
          {t(`portable.verdict.${pre.verdict}` as Key)}
        </p>
        {pre.projection && <Projection from={pre.projection[0]} to={pre.projection[1]} cap={realm.level_cap} />}
        {pre.verdict === "degraded" && <p className="mt-3 text-sm text-muted">{t("portable.degraded.text")}</p>}
        <Notes notes={pre.notes} />
        <p className="mt-3 text-sm text-muted">{t(`portable.step.${pre.step}` as Key)}</p>
        {pre.step === "resolve" && <div className="mt-3 flex flex-wrap gap-2">
          <Button size="sm" variant="primary" disabled={busy} onClick={() => void resolve("use_canonical")}>{t("portable.resolve.canonical")}</Button>
          <Button size="sm" disabled={busy} onClick={() => void resolve("detach")}>{t("portable.resolve.detach")}</Button>
          <Button size="sm" variant="ghost" disabled={busy} onClick={onClose}>{t("portable.cancel")}</Button>
        </div>}
        {pre.step === "resolve" && <p className="mt-2 text-xs text-muted">{t("portable.resolve.help")}</p>}
        {(pre.needs_account || realm.game_account === null) && pre.step === "prepare" && <div className="mt-4">
          <label htmlFor="play-account" className="block text-sm font-medium">{t("portable.play.account")}</label>
          <p className="text-xs text-muted">{t("portable.play.accountHint")}</p>
          <input id="play-account" value={account} onChange={(e) => setAccount(e.target.value)} disabled={busy} className="mt-1 w-full rounded-md border border-line bg-card px-3 py-2" autoComplete="off" />
        </div>}
      </>}
      {done && <div className="mt-4">
        <p className="flex items-center gap-2 text-sm font-medium text-ok"><CheckCircle2 className="h-4 w-4" aria-hidden />{t("portable.play.ready")}</p>
        {done.projection && <Projection from={done.projection[0]} to={done.projection[1]} cap={realm.level_cap} />}
        <Notes notes={done.notes} />
        <p className="mt-3 text-sm text-muted">{t("portable.play.next", { address: done.realm_address })}</p>
      </div>}
      {error && <ErrorLine error={error} />}
      <div className="mt-5 flex justify-end gap-2">
        <Button variant="ghost" onClick={onClose}>{done ? t("portable.close") : t("portable.cancel")}</Button>
        {!done && <Button variant="primary" disabled={blocked || (pre?.step === "prepare" && !account.trim() && realm.game_account === null)} onClick={() => void play()}>
          {busy && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}{busy ? t("portable.status.preparing") : t("portable.play.go")}
        </Button>}
        {done && <Button variant="primary" disabled={launching} onClick={() => void launch()}>{launching && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}{t("portable.play.launch")}</Button>}
      </div>
    </Card>
  </div>;
}

// ---- Make portable ------------------------------------------------------------------------------------------------------------

function MakeDialog({ realms, onClose, onChanged }: { realms: RealmView[]; onClose: () => void; onChanged: () => void }) {
  const t = useT();
  const usable = realms.filter((r) => r.database_ok);
  const [realmId, setRealmId] = useState(usable[0]?.id ?? "");
  const [list, setList] = useState<LocalCharacterView[] | null>(null);
  const [error, setError] = useState<PortableError | null>(null);
  const [busy, setBusy] = useState<number | null>(null);
  const [made, setMade] = useState<string | null>(null);
  useEffect(() => {
    if (!realmId) return;
    let alive = true;
    setList(null); setError(null);
    void portable.localCharacters(realmId).then((l) => { if (alive) setList(l); }).catch((e) => { if (alive) setError(asPortableError(e)); });
    return () => { alive = false; };
  }, [realmId]);
  async function make(c: LocalCharacterView) {
    setBusy(c.token); setError(null);
    try { const v = await portable.make(realmId, c.token); setMade(v.name); onChanged(); setList((l) => l?.filter((x) => x.token !== c.token) ?? null); }
    catch (e) { setError(asPortableError(e)); } finally { setBusy(null); }
  }
  return <div className="fixed inset-0 z-40 flex items-center justify-center bg-black/60 p-6" role="dialog" aria-modal="true" aria-labelledby="make-title">
    <Card className="max-h-full w-full max-w-2xl overflow-y-auto p-6">
      <div className="flex items-start justify-between">
        <h2 id="make-title" className="text-lg font-semibold">{t("portable.make.title")}</h2>
        <button onClick={onClose} aria-label={t("portable.close")} className="cursor-pointer text-muted hover:text-ink"><X className="h-5 w-5" /></button>
      </div>
      <p className="mt-1 text-sm text-muted">{t("portable.make.text")}</p>
      {usable.length === 0 ? <p className="mt-4 text-sm text-warn">{t("portable.make.noRealm")}</p> : <>
        <label htmlFor="make-realm" className="mt-4 block text-sm font-medium">{t("portable.play.realm")}</label>
        <select id="make-realm" value={realmId} onChange={(e) => setRealmId(e.target.value)} className="mt-1 w-full rounded-md border border-line bg-card px-3 py-2">
          {usable.map((r) => <option key={r.id} value={r.id}>{r.name}</option>)}
        </select>
        {!list && !error && <p className="mt-4 flex items-center gap-2 text-sm text-muted"><Loader2 className="h-4 w-4 animate-spin" aria-hidden />{t("portable.make.loading")}</p>}
        {list && list.length === 0 && <p className="mt-4 text-sm text-muted">{t("portable.make.none")}</p>}
        {list && list.length > 0 && <ul className="mt-4 divide-y divide-line rounded-md border border-line">
          {list.map((c) => <li key={c.token} className="flex items-center gap-3 px-3 py-2">
            <div className="min-w-0 flex-1"><p className="truncate font-medium">{c.name}</p><p className="text-xs text-muted">{c.class_name} · {t("portable.level", { level: c.level })}{c.account ? ` · ${c.account}` : ""}</p>
              {!c.eligible && <p className="text-xs text-warn">{c.reasons.map((r) => t(`portable.blocker.${r}` as Key)).join(", ")}</p>}</div>
            <Button size="sm" disabled={!c.eligible || busy !== null} onClick={() => void make(c)}>{busy === c.token && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}{t("portable.make.do")}</Button>
          </li>)}
        </ul>}
      </>}
      {made && <p className="mt-3 text-sm text-ok" role="status">{t("portable.make.done", { name: made })}</p>}
      {error && <ErrorLine error={error} />}
    </Card>
  </div>;
}

// ---- History ------------------------------------------------------------------------------------------------------------------

function HistoryDialog({ character, onClose }: { character: CharacterView; onClose: () => void }) {
  const t = useT();
  const [list, setList] = useState<HistoryEntry[] | null>(null);
  const [error, setError] = useState<PortableError | null>(null);
  useEffect(() => { void portable.history(character.id).then(setList).catch((e) => setError(asPortableError(e))); }, [character.id]);
  return <div className="fixed inset-0 z-40 flex items-center justify-center bg-black/60 p-6" role="dialog" aria-modal="true" aria-labelledby="hist-title">
    <Card className="max-h-full w-full max-w-xl overflow-y-auto p-6">
      <div className="flex items-start justify-between">
        <h2 id="hist-title" className="text-lg font-semibold">{t("portable.history.title", { name: character.name })}</h2>
        <button onClick={onClose} aria-label={t("portable.close")} className="cursor-pointer text-muted hover:text-ink"><X className="h-5 w-5" /></button>
      </div>
      {!list && !error && <Loader2 className="mt-4 h-4 w-4 animate-spin" aria-hidden />}
      {list && <ul className="mt-4 divide-y divide-line text-sm">
        {list.map((h) => <li key={h.revision} className="flex gap-3 py-2"><span className="w-12 shrink-0 font-medium">#{h.revision}</span><span className="flex-1 text-muted">{when(h.at)} · {t(`portable.history.${h.kind}` as Key, { realm: h.source_realm })}</span></li>)}
      </ul>}
      {error && <ErrorLine error={error} />}
    </Card>
  </div>;
}

// ---- the page ----------------------------------------------------------------------------------------------------------------

export function PortablePage({ onOpenSettings }: { onOpenSettings?: () => void }) {
  const t = useT();
  const { state, refresh } = usePortable();
  const [playing, setPlaying] = useState<{ character: CharacterView; realm: string } | null>(null);
  const [history, setHistory] = useState<CharacterView | null>(null);
  const [making, setMaking] = useState(false);
  const [error, setError] = useState<PortableError | null>(null);
  const [copied, setCopied] = useState(false);
  const realms = state?.realms ?? [];
  const characters = state?.characters ?? [];
  const usable = useMemo(() => realms, [realms]);

  async function addRealm() {
    setError(null);
    try {
      const file = await open({ multiple: false, filters: [{ name: t("portable.realm.fileType"), extensions: ["json"] }] });
      if (typeof file === "string") { await portable.addRealm(file); await refresh(); }
    } catch (e) { setError(asPortableError(e)); }
  }
  async function removeRealm(id: string) {
    setError(null);
    try { await portable.removeRealm(id); await refresh(); } catch (e) { setError(asPortableError(e)); }
  }
  async function copyDiagnostics() {
    try {
      const json = JSON.stringify(await portable.diagnostics(), null, 2);
      await navigator.clipboard.writeText(json);
      setCopied(true); setTimeout(() => setCopied(false), 2500);
    } catch (e) { setError(asPortableError(e)); }
  }

  return <div>
    <div className="flex items-center justify-between">
      <h1 className="text-2xl font-semibold">{t("portable.title")}</h1>
      <div className="flex gap-2">
        <Button size="sm" onClick={() => setMaking(true)} disabled={!realms.some((r) => r.database_ok)}><Plus className="h-4 w-4" aria-hidden />{t("portable.make.button")}</Button>
      </div>
    </div>
    <p className="mt-2 text-muted">{t("portable.intro")}</p>
    {state && !state.runtime.running && <p className="mt-3 text-sm text-warn" role="status">{t("portable.runtime.stopped")}</p>}
    {error && <ErrorLine error={error} />}

    <h2 className="mt-8 text-lg font-semibold">{t("portable.realms.title")}</h2>
    <Card className="mt-3 divide-y divide-line">
      {usable.length === 0 && <p className="p-5 text-sm text-muted">{t("portable.realms.none")}</p>}
      {usable.map((r) => <div key={r.id} className="flex items-center gap-3 px-5 py-3" data-realm={r.id}>
        <span className={cn("h-2.5 w-2.5 rounded-full", r.online ? "bg-ok" : r.database_ok ? "bg-warn" : "bg-bad")} aria-hidden />
        <div className="min-w-0 flex-1">
          <p className="truncate font-medium">{r.name}</p>
          <p className="text-xs text-muted">
            {t(r.online ? "portable.realm.online" : r.database_ok ? "portable.realm.stopped" : "portable.realm.unreachable")}
            {r.level_cap ? ` · ${t("portable.realm.cap", { cap: r.level_cap })}` : ""}
            {r.portable === "setup" ? ` · ${t("portable.realm.setup")}` : ""}
            {` · ${t(r.kind === "installed" ? "portable.realm.installed" : "portable.realm.prepared")}`}
          </p>
        </div>
        {r.kind === "prepared" && <Button size="sm" variant="ghost" onClick={() => void removeRealm(r.id)} aria-label={t("portable.realm.remove")}><Trash2 className="h-4 w-4" aria-hidden /></Button>}
      </div>)}
      <div className="px-5 py-3"><Button size="sm" variant="ghost" onClick={() => void addRealm()}><Plus className="h-4 w-4" aria-hidden />{t("portable.realm.add")}</Button></div>
    </Card>

    <h2 className="mt-8 text-lg font-semibold">{t("portable.characters.title")}</h2>
    {characters.length === 0 && <Card className="mt-3 p-5 text-sm text-muted">{t("portable.characters.none")}</Card>}
    <div className="mt-3 space-y-4">
      {characters.map((c) => <Card key={c.id} className="p-5" data-character={c.name}>
        <div className="flex flex-wrap items-center gap-3">
          <Swords className="h-5 w-5 text-gold" aria-hidden />
          <div className="min-w-0 flex-1">
            <p className="text-lg font-semibold">{c.name}</p>
            <p className="text-sm text-muted">{c.class_name} · {t("portable.level", { level: c.level })} · {t("portable.revision", { n: c.revision })}</p>
            <p className="text-xs text-muted">{t("portable.lastSync", { when: when(c.updated_at) })}{c.active_realm ? ` · ${t("portable.activeOn", { realm: c.active_realm })}` : ""}</p>
          </div>
          <StatusChip status={c.status} />
        </div>
        {c.copies.length > 0 && <ul className="mt-4 divide-y divide-line rounded-md border border-line text-sm">
          {c.copies.map((x) => <li key={x.realm_id} className="flex flex-wrap items-center gap-3 px-3 py-2">
            <span className="min-w-0 flex-1 truncate">{x.realm_name}</span>
            {x.projected_level && <span className="text-xs text-muted">{t("portable.copy.projected", { level: x.projected_level })}</span>}
            <span className="text-xs text-muted">{t("portable.copy.synced", { n: x.synced_revision })}</span>
            <StatusChip status={x.status} degraded={x.degraded} />
          </li>)}
        </ul>}
        <div className="mt-4 flex flex-wrap gap-2">
          <Button variant="primary" size="sm" disabled={realms.length === 0 || LIVE.includes(c.status) && c.status !== "waiting_login"} onClick={() => setPlaying({ character: c, realm: c.copies.find((x) => x.status === "waiting_login")?.realm_id ?? c.copies[0]?.realm_id ?? realms[0]!.id })}>{t("portable.play.button")}</Button>
          <Button size="sm" variant="ghost" onClick={() => setHistory(c)}><History className="h-4 w-4" aria-hidden />{t("portable.history.button")}</Button>
        </div>
      </Card>)}
    </div>

    <h2 className="mt-8 text-lg font-semibold">{t("portable.diag.title")}</h2>
    <Card className="mt-3 p-5">
      <p className="text-sm text-muted">{t("portable.diag.text")}</p>
      {state && state.runtime.errors.length > 0 && <ul className="mt-3 space-y-1 text-xs text-bad">{state.runtime.errors.slice(-5).map((e, i) => <li key={i} className="selectable">{when(e.at)} · {e.message}</li>)}</ul>}
      <div className="mt-3 flex items-center gap-3"><Button size="sm" onClick={() => void copyDiagnostics()}>{t("portable.diag.copy")}</Button>{copied && <span className="text-sm text-ok" role="status">{t("portable.diag.copied")}</span>}</div>
    </Card>

    {playing && <PlayDialog character={playing.character} realms={realms} initial={playing.realm} onClose={() => setPlaying(null)} onChanged={() => void refresh()} />}
    {making && <MakeDialog realms={realms} onClose={() => setMaking(false)} onChanged={() => void refresh()} />}
    {history && <HistoryDialog character={history} onClose={() => setHistory(null)} />}
    {onOpenSettings && null}
  </div>;
}
