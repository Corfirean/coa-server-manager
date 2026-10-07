import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ArrowDown, ArrowUp, Loader2, RefreshCw, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { useI18n, type Key } from "@/i18n";
import { asUiError } from "@/lib/api";
import { realmRegistry, useRegistryStatus } from "@/lib/registry";
import { asPortableError, portable, usePortable, type PreflightView } from "@/lib/portable";
import { Notes, Projection } from "@/screens/PortablePage";
import { browse, rate, type BrowseParams, type ModuleEntry, type ModuleInfo, type RealmDetail, type RealmSummary, type SortKey } from "@/lib/browse";

const VISIBLE_MODULES = 3;
const PAGE = 50;

type Catalog = Record<string, ModuleInfo>;

/** A module chip. A module the local catalog knows gets its name and a tooltip from the catalog; any other id is shown as the plain text it is. */
function ModuleChip({ m, catalog }: { m: ModuleEntry; catalog: Catalog }) {
  const { locale } = useI18n();
  const known = catalog[m.id];
  const name = known?.name ?? m.id;
  const description = known ? known.description[locale] ?? known.description.en ?? "" : "";
  return <span className="group relative inline-block" tabIndex={0}>
    <span className={cn("inline-flex items-center rounded-full border px-2 py-0.5 text-xs", m.enabled ? "border-line bg-white/5 text-ink" : "border-line/60 text-muted line-through")}>{name}</span>
    <span role="tooltip" className="pointer-events-none absolute left-0 top-full z-30 mt-1 hidden w-64 rounded-md border border-line bg-card p-3 text-left text-xs shadow-lg group-hover:block group-focus:block">
      <span className="block text-sm font-semibold text-ink">{name}</span>
      {description && <span className="mt-1 block text-muted">{description}</span>}
      {m.version && <span className="mt-1 block text-muted">{m.version}</span>}
      {!known && <span className="mt-1 block text-muted">{m.id}</span>}
    </span>
  </span>;
}

function Modules({ modules, catalog }: { modules: ModuleEntry[]; catalog: Catalog }) {
  const { t } = useI18n();
  const on = modules.filter((m) => m.enabled);
  if (on.length === 0) return <span className="text-muted">—</span>;
  return <span className="flex flex-wrap items-center gap-1">
    {on.slice(0, VISIBLE_MODULES).map((m) => <ModuleChip key={m.id} m={m} catalog={catalog} />)}
    {on.length > VISIBLE_MODULES && <span className="text-xs text-muted" title={on.slice(VISIBLE_MODULES).map((m) => catalog[m.id]?.name ?? m.id).join(", ")}>{t("browse.more", { n: on.length - VISIBLE_MODULES })}</span>}
  </span>;
}

function Online({ p }: { p: RealmSummary["population"] }) {
  const { tn } = useI18n();
  return <span className="whitespace-nowrap">{tn("browse.players", p.players)}<span className="text-muted"> + </span>{tn("browse.bots", p.bots)}</span>;
}

function Mode({ ruleset }: { ruleset: string }) {
  return <span>{ruleset === "wildcard" ? "Wildcard" : "CoA"}</span>;
}

function compactRates(r: RealmSummary["rates"], t: (k: Key, v?: Record<string, string | number>) => string): string {
  return `${t("browse.rate.xp")} ${rate(r.xp_kill)} · ${t("browse.rate.loot")} ${rate(r.loot)}`;
}

const RATE_ROWS: [keyof RealmSummary["rates"], Key][] = [["xp_kill", "browse.rate.xp"], ["xp_quest", "browse.rate.xpQuest"], ["xp_explore", "browse.rate.xpExplore"], ["loot", "browse.rate.loot"], ["money", "browse.rate.money"], ["reputation", "browse.rate.reputation"], ["honor", "browse.rate.honor"]];

function SortHeader({ id, sort, order, onSort, children, className }: { id: SortKey; sort: SortKey; order: "asc" | "desc"; onSort: (k: SortKey) => void; children: React.ReactNode; className?: string }) {
  const active = sort === id;
  return <th scope="col" aria-sort={active ? (order === "asc" ? "ascending" : "descending") : "none"} className={cn("px-3 py-2 text-left font-medium", className)}>
    <button onClick={() => onSort(id)} className={cn("inline-flex cursor-pointer items-center gap-1 hover:text-ink", active ? "text-ink" : "text-muted")}>{children}{active && (order === "asc" ? <ArrowUp className="h-3.5 w-3.5" aria-hidden /> : <ArrowDown className="h-3.5 w-3.5" aria-hidden />)}</button>
  </th>;
}

function Drawer({ summary, catalog, onClose }: { summary: RealmSummary; catalog: Catalog; onClose: () => void }) {
  const { t } = useI18n();
  const { state } = usePortable();
  const [detail, setDetail] = useState<RealmDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [character, setCharacter] = useState("");
  const [pre, setPre] = useState<PreflightView | null>(null);
  const [preError, setPreError] = useState<string | null>(null);
  const characters = state?.characters ?? [];

  useEffect(() => {
    let alive = true;
    setDetail(null); setError(null);
    void browse.detail(summary.realm_id).then((d) => { if (alive) setDetail(d); }).catch((e) => { if (alive) setError(asUiError(e).technical); });
    return () => { alive = false; };
  }, [summary.realm_id]);
  useEffect(() => { if (!character && characters.length > 0) setCharacter(characters[0].id); }, [characters.length]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    if (!character || !detail) { setPre(null); return; }
    let alive = true;
    setPreError(null);
    void portable.preflightRemote(character, detail.capabilities).then((p) => { if (alive) setPre(p); }).catch((e) => { if (alive) { setPre(null); setPreError(asPortableError(e).message); } });
    return () => { alive = false; };
  }, [character, detail?.realm_id, detail?.metadata_revision]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") onClose(); };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const d = detail;
  const chosen = characters.find((c) => c.id === character);
  const blocked = pre?.verdict === "incompatible";
  return <aside role="complementary" aria-label={summary.display_name} className="fixed inset-y-0 right-0 z-40 flex w-[440px] max-w-full flex-col border-l border-line bg-[#0e1013] shadow-2xl">
    <div className="flex items-start justify-between gap-3 border-b border-line p-5">
      <div className="min-w-0">
        <h2 className="truncate text-lg font-semibold" data-testid="drawer-title">{summary.display_name}</h2>
        <p className="mt-0.5 text-sm text-muted"><Mode ruleset={summary.ruleset} /> · {t("browse.cap")} {summary.level_cap ?? "—"}</p>
        <p className="text-sm"><Online p={summary.population} />{summary.population.capacity !== null && <span className="text-muted"> · {t("browse.capacity", { n: summary.population.capacity })}</span>}</p>
      </div>
      <button onClick={onClose} aria-label={t("portable.close")} className="cursor-pointer text-muted hover:text-ink"><X className="h-5 w-5" /></button>
    </div>
    <div className="flex-1 space-y-5 overflow-y-auto p-5 text-sm">
      {summary.description && <p className="whitespace-pre-wrap break-words text-muted">{d?.listing.description ?? summary.description}</p>}
      <section>
        <h3 className="mb-1 font-medium">{t("browse.rates")}</h3>
        <dl className="grid grid-cols-2 gap-x-4 gap-y-0.5">
          {RATE_ROWS.map(([k, label]) => <div key={k} className="flex justify-between border-b border-line/40 py-0.5"><dt className="text-muted">{t(label)}</dt><dd>{rate(summary.rates[k])}</dd></div>)}
        </dl>
      </section>
      <section>
        <h3 className="mb-1 font-medium">{t("browse.modules")}</h3>
        {summary.modules.length === 0 ? <p className="text-muted">—</p> : <ul className="space-y-1.5">
          {summary.modules.map((m) => <li key={m.id} className="flex items-baseline justify-between gap-3"><span className={cn(!m.enabled && "text-muted line-through")}>{catalog[m.id]?.name ?? m.id}</span><span className="text-xs text-muted">{m.version ?? ""}{!m.enabled && ` ${t("browse.moduleOff")}`}</span></li>)}
        </ul>}
      </section>
      <section>
        <h3 className="mb-1 font-medium">{t("browse.accounts")}</h3>
        <p className="text-muted">{summary.account_provisioning.automatic ? t("browse.accountsAutomatic") : t("browse.accountsExisting")}</p>
      </section>
      <section>
        <h3 className="mb-1 font-medium">{t("browse.compat")}</h3>
        {error && <p className="text-bad" role="alert">{error}</p>}
        {!error && !d && <p className="flex items-center gap-2 text-muted"><Loader2 className="h-4 w-4 animate-spin" aria-hidden />{t("portable.make.loading")}</p>}
        {characters.length === 0 && <p className="text-muted">{t("browse.noCharacter")}</p>}
        {characters.length > 0 && <>
          <select aria-label={t("browse.character")} value={character} onChange={(e) => setCharacter(e.target.value)} className="w-full rounded-md border border-line bg-card px-3 py-2">
            {characters.map((c) => <option key={c.id} value={c.id}>{c.name} — {c.class_name}, {t("portable.level", { level: c.level })}</option>)}
          </select>
          {preError && <p className="mt-2 text-warn">{preError}</p>}
          {pre && chosen && <div className={cn("mt-2 rounded-md border p-3", blocked ? "border-bad/50 bg-bad/10" : pre.verdict === "degraded" ? "border-warn/50 bg-warn/10" : "border-ok/40 bg-ok/10")} data-verdict={pre.verdict}>
            <p className="font-medium">{blocked ? t("browse.cannotPlay") : t("browse.canPlay")}</p>
            {pre.projection && <Projection from={pre.projection[0]} to={pre.projection[1]} cap={summary.level_cap} />}
            <Notes notes={pre.notes} />
          </div>}
        </>}
      </section>
    </div>
    <div className="border-t border-line p-5">
      <Button variant="primary" className="w-full" disabled>{t("browse.play")}</Button>
      <p className="mt-2 text-xs text-muted">{t("browse.playSoon")}</p>
    </div>
  </aside>;
}

export function BrowsePage() {
  const { t } = useI18n();
  const { status, refresh } = useRegistryStatus(5000);
  const [url, setUrl] = useState("");
  const [q, setQ] = useState("");
  const [ruleset, setRuleset] = useState<"" | "coa" | "wildcard">("");
  const [capMin, setCapMin] = useState("");
  const [capMax, setCapMax] = useState("");
  const [module, setModule] = useState("");
  const [playersMin, setPlayersMin] = useState("");
  const [sort, setSort] = useState<SortKey>("players");
  const [order, setOrder] = useState<"asc" | "desc">("desc");
  const [rows, setRows] = useState<RealmSummary[]>([]);
  const [next, setNext] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<RealmSummary | null>(null);
  const [catalog, setCatalog] = useState<Catalog>({});
  const ticket = useRef(0);

  useEffect(() => { void browse.modules().then((m) => setCatalog(Object.fromEntries(m.map((x) => [x.id, x])))).catch(() => {}); }, []);
  useEffect(() => { if (status?.url && url === "") setUrl(status.url); }, [status?.url]); // eslint-disable-line react-hooks/exhaustive-deps

  const params = useMemo<BrowseParams>(() => ({
    q: q.trim() || undefined,
    ruleset: ruleset || undefined,
    cap_min: capMin !== "" && !Number.isNaN(Number(capMin)) ? Number(capMin) : undefined,
    cap_max: capMax !== "" && !Number.isNaN(Number(capMax)) ? Number(capMax) : undefined,
    module: module || undefined,
    players_min: playersMin !== "" && !Number.isNaN(Number(playersMin)) ? Number(playersMin) : undefined,
    sort, order, limit: PAGE,
  }), [q, ruleset, capMin, capMax, module, playersMin, sort, order]);

  const load = useCallback(async (cursor?: string) => {
    const mine = ++ticket.current;
    setLoading(true);
    setError(null);
    try {
      const page = await browse.list({ ...params, cursor });
      if (mine !== ticket.current) return;
      setRows((old) => (cursor ? [...old, ...page.realms] : page.realms));
      setNext(page.next_cursor);
    } catch (e) {
      if (mine !== ticket.current) return;
      setError(asUiError(e).technical);
      if (!cursor) { setRows([]); setNext(null); }
    } finally { if (mine === ticket.current) setLoading(false); }
  }, [params]);

  useEffect(() => {
    if (!status?.url) return;
    const timer = setTimeout(() => void load(), q ? 300 : 0);
    return () => clearTimeout(timer);
  }, [load, status?.url]); // eslint-disable-line react-hooks/exhaustive-deps

  function onSort(k: SortKey) {
    if (k === sort) setOrder((o) => (o === "asc" ? "desc" : "asc"));
    else { setSort(k); setOrder(k === "name" ? "asc" : "desc"); }
  }

  const moduleChoices = useMemo(() => Object.values(catalog).filter((m) => m.id !== "client-compat").sort((a, b) => a.name.localeCompare(b.name)), [catalog]);
  const noUrl = !status?.url;

  return <div className="relative">
    <div className="flex items-start justify-between gap-4">
      <div>
        <h1 className="text-2xl font-semibold">{t("browse.title")}</h1>
        <p className="mt-1 text-sm text-muted">{t("browse.text")}</p>
      </div>
      <Button size="sm" onClick={() => void load()} disabled={loading || noUrl}>{loading ? <Loader2 className="h-4 w-4 animate-spin" aria-hidden /> : <RefreshCw className="h-4 w-4" aria-hidden />}{t("browse.refresh")}</Button>
    </div>

    <div className="mt-4 flex gap-2">
      <input aria-label={t("registry.url")} value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://registry.example" className="min-w-0 flex-1 rounded-md border border-line bg-card px-3 py-2 text-sm" />
      <Button size="sm" disabled={url.trim() === (status?.url ?? "")} onClick={() => void realmRegistry.setUrl(url.trim() || null).then(refresh).catch((e) => setError(asUiError(e).technical))}>{t("registry.urlSave")}</Button>
    </div>
    {noUrl && <p className="mt-2 text-sm text-muted">{t("browse.noUrl")}</p>}

    <div className="mt-4 grid grid-cols-2 gap-3 md:grid-cols-6">
      <input aria-label={t("browse.search")} value={q} onChange={(e) => setQ(e.target.value)} placeholder={t("browse.search")} className="col-span-2 rounded-md border border-line bg-card px-3 py-2 text-sm" />
      <select aria-label={t("browse.mode")} value={ruleset} onChange={(e) => setRuleset(e.target.value as "" | "coa" | "wildcard")} className="rounded-md border border-line bg-card px-3 py-2 text-sm">
        <option value="">{t("browse.mode")}: {t("browse.any")}</option><option value="coa">CoA</option><option value="wildcard">Wildcard</option>
      </select>
      <div className="flex items-center gap-1 text-sm"><input aria-label={t("browse.capMin")} inputMode="numeric" value={capMin} onChange={(e) => setCapMin(e.target.value.replace(/\D/g, ""))} placeholder={t("browse.capMin")} className="w-full rounded-md border border-line bg-card px-2 py-2" /><span className="text-muted">–</span><input aria-label={t("browse.capMax")} inputMode="numeric" value={capMax} onChange={(e) => setCapMax(e.target.value.replace(/\D/g, ""))} placeholder={t("browse.capMax")} className="w-full rounded-md border border-line bg-card px-2 py-2" /></div>
      <select aria-label={t("browse.module")} value={module} onChange={(e) => setModule(e.target.value)} className="rounded-md border border-line bg-card px-3 py-2 text-sm">
        <option value="">{t("browse.module")}: {t("browse.any")}</option>
        {moduleChoices.map((m) => <option key={m.id} value={m.id}>{m.name}</option>)}
      </select>
      <input aria-label={t("browse.playersMin")} inputMode="numeric" value={playersMin} onChange={(e) => setPlayersMin(e.target.value.replace(/\D/g, ""))} placeholder={t("browse.playersMin")} className="rounded-md border border-line bg-card px-3 py-2 text-sm" />
    </div>

    {error && <p className="mt-3 text-sm text-bad" role="alert">{error}</p>}

    <div className="mt-4 overflow-x-auto rounded-lg border border-line">
      <table className="w-full min-w-[760px] text-sm" data-testid="realm-table">
        <thead className="bg-white/[0.03] text-xs uppercase tracking-wide">
          <tr>
            <SortHeader id="name" sort={sort} order={order} onSort={onSort}>{t("browse.col.server")}</SortHeader>
            <th scope="col" className="px-3 py-2 text-left font-medium text-muted">{t("browse.col.modules")}</th>
            <SortHeader id="cap" sort={sort} order={order} onSort={onSort}>{t("browse.col.cap")}</SortHeader>
            <th scope="col" className="px-3 py-2 text-left font-medium text-muted">{t("browse.col.rates")}</th>
            <th scope="col" className="px-3 py-2 text-left font-medium text-muted">{t("browse.col.mode")}</th>
            <th scope="col" className="px-3 py-2 text-left font-medium text-muted" title={t("browse.pingHint")}>{t("browse.col.ping")}</th>
            <SortHeader id="players" sort={sort} order={order} onSort={onSort}>{t("browse.col.online")}</SortHeader>
          </tr>
        </thead>
        <tbody>
          {rows.map((r) => <tr key={r.realm_id} onClick={() => setSelected(r)} onKeyDown={(e) => { if (e.key === "Enter") setSelected(r); }} tabIndex={0} aria-selected={selected?.realm_id === r.realm_id} className={cn("cursor-pointer border-t border-line/60 hover:bg-white/[0.04] focus:bg-white/[0.06] focus:outline-none", selected?.realm_id === r.realm_id && "bg-white/[0.06]")}>
            <td className="max-w-[200px] px-3 py-2"><div className="truncate font-medium" title={r.display_name}>{r.display_name}</div><div className="truncate text-xs text-muted">{[r.region, r.language].filter(Boolean).join(" · ")}</div></td>
            <td className="px-3 py-2"><Modules modules={r.modules} catalog={catalog} /></td>
            <td className="px-3 py-2">{r.level_cap ?? "—"}</td>
            <td className="whitespace-nowrap px-3 py-2">{compactRates(r.rates, t)}</td>
            <td className="px-3 py-2"><Mode ruleset={r.ruleset} /></td>
            <td className="px-3 py-2 text-muted" title={t("browse.pingHint")}>—</td>
            <td className="px-3 py-2"><Online p={r.population} /></td>
          </tr>)}
          {rows.length === 0 && !loading && <tr><td colSpan={7} className="px-3 py-8 text-center text-muted">{noUrl ? t("browse.noUrl") : t("browse.empty")}</td></tr>}
        </tbody>
      </table>
    </div>
    <div className="mt-3 flex items-center gap-3 text-sm text-muted">
      {loading && <span className="flex items-center gap-2"><Loader2 className="h-4 w-4 animate-spin" aria-hidden />{t("portable.make.loading")}</span>}
      {!loading && rows.length > 0 && <span>{t("browse.shown", { n: rows.length })}</span>}
      {next && !loading && <Button size="sm" onClick={() => void load(next)}>{t("browse.more.button")}</Button>}
    </div>
    {selected && <Drawer summary={selected} catalog={catalog} onClose={() => setSelected(null)} />}
  </div>;
}
