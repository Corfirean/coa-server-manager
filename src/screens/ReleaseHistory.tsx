import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import { useI18n, LOCALES, type Locale } from "@/i18n";
import { useSelfUpdate } from "@/lib/selfUpdate";
import { bundledHistories, compareVersions, releasesForUpdate, repositories, translatedNotes, type Component, type History } from "@/lib/releaseHistory";
import { api } from "@/lib/api";
import { Card } from "@/components/ui/card";

export interface HistorySelection { component: Component; from?: string; to?: string }
export function ReleaseHistory({ selection, onSelect }: { selection: HistorySelection; onSelect: (selection: HistorySelection) => void }) {
  const { locale, t } = useI18n();
  const [language, setLanguage] = useState<Locale>(locale);
  const [feeds, setFeeds] = useState(bundledHistories);
  const [fresh, setFresh] = useState<Partial<Record<Component, boolean>>>({});
  const [mode, setMode] = useState<"all" | "range">("all");
  const [selected, setSelected] = useState("");
  const [installed, setInstalled] = useState<string>();
  const updater = useSelfUpdate();
  const { component } = selection;
  const lastServer = useRef<HistorySelection>({ component: "server" });
  const [linkError, setLinkError] = useState<string>();
  useEffect(() => { if (component === "server") lastServer.current = selection; setLinkError(undefined); }, [selection, component]);
  const target = component === "manager" ? updater.version : selection.to;
  const from = component === "manager" ? installed : selection.from;
  useEffect(() => setLanguage(locale), [locale]);
  useEffect(() => { void getVersion().then(setInstalled).catch(() => {}); }, []);
  useEffect(() => {
    setSelected("");
    setMode(selection.to || (component === "manager" && updater.version) ? "range" : "all");
  }, [selection, component, updater.version]);
  useEffect(() => {
    let live = true;
    void invoke<History>("release_history", { component, target }).then(feed => {
      if (live && feed.component === component && Array.isArray(feed.releases)) {
        setFeeds(previous => ({ ...previous, [component]: feed }));
        setFresh(previous => ({ ...previous, [component]: feed.source === "remote" }));
      }
    }).catch(() => {}); // Bundled reviewed notes remain visible offline.
    return () => { live = false; };
  }, [component, target]);
  const releases = [...(mode === "range" ? releasesForUpdate(feeds[component].releases, from, target) : feeds[component].releases)]
    .sort((a, b) => compareVersions(b.version.split("-")[0], a.version.split("-")[0]) || b.date.localeCompare(a.date));
  const record = releases.find(r => r.version === selected) ?? releases[0];
  const notes = record ? translatedNotes(component, record, language) : undefined;
  return <Card id="release-history" tabIndex={-1} className="mt-6 scroll-mt-4 overflow-hidden">
    <div className="flex flex-wrap items-center justify-between gap-4 border-b border-line p-5">
      <h2 className="font-semibold">{t("history.title")}</h2>
      <div className="flex rounded-md border border-line p-1" aria-label={t("history.title")}>
        {(["server", "manager"] as const).map(c => <button key={c} aria-pressed={component === c} onClick={() => onSelect(c === "server" ? lastServer.current : { component: c })} className={`cursor-pointer rounded px-3 py-1.5 text-sm ${component === c ? "bg-gold/10 text-gold" : "text-muted hover:text-ink"}`}>{t(`history.${c}`)}</button>)}
      </div>
    </div>
    <div className="flex flex-wrap items-center justify-between gap-3 border-b border-line px-5 py-3 text-sm">
      <div className="flex gap-3">
        <button className={mode === "all" ? "text-gold" : "cursor-pointer text-muted"} aria-pressed={mode === "all"} onClick={() => setMode("all")}>{t("history.all")}</button>
        {target && <button className={mode === "range" ? "text-gold" : "cursor-pointer text-muted"} aria-pressed={mode === "range"} onClick={() => setMode("range")}>{t("history.range")}{from ? ` · ${from} → ${target}` : ` · ${target}`}</button>}
      </div>
      <label className="flex items-center gap-2 text-muted">{t("history.language")}<select value={language} onChange={e => setLanguage(e.target.value as Locale)} className="rounded border border-line bg-panel px-2 py-1 text-ink">{LOCALES.map(l => <option key={l.id} value={l.id}>{l.name}</option>)}</select></label>
    </div>
    <div className="grid min-h-64 md:grid-cols-[180px_1fr]">
      <div className="flex max-h-96 gap-1 overflow-auto border-b border-line p-3 md:block md:border-r md:border-b-0" aria-label={t("history.all")}>
        {releases.map(r => <button key={r.tag} aria-pressed={r.version === record?.version} onClick={() => setSelected(r.version)} className={`mb-1 block shrink-0 cursor-pointer rounded-md px-3 py-2 text-left text-sm md:w-full ${r.version === record?.version ? "bg-gold/10 text-gold" : "text-muted hover:bg-white/5"}`}><span className="block font-medium">{r.version}</span><span className="text-xs text-muted">{r.date}{r.prerelease ? " · Pre-release" : ""}</span></button>)}
      </div>
      <div className="min-w-0 p-5" aria-live="polite">
        {record && notes ? <><h3 className="text-lg font-semibold">{t(`history.${component}`)} {record.version}</h3>
          {notes.fallback && <p className="mt-2 text-xs text-warn">{t("history.fallback")}</p>}
          <ul lang={notes.fallback ? "en" : language} className="mt-4 list-disc space-y-2 pl-5 text-sm leading-relaxed">{notes.notes.map((note, i) => <li key={i}>{note}</li>)}</ul>
          {record.experimentalFeatures?.includes("portable-characters") && <p className="mt-4 rounded border border-warn/30 p-3 text-sm text-warn">{t("history.experimental")}</p>}
          <a className="mt-5 inline-block text-sm text-gold hover:underline" href={`https://github.com/${repositories[component]}/releases/tag/${encodeURIComponent(record.tag)}`} onClick={e => { e.preventDefault(); void api.openLink(e.currentTarget.href).catch(error => setLinkError(String(error))); }}>GitHub ↗</a>
          {linkError && <p className="mt-2 break-words text-xs text-bad" role="alert">{linkError}</p>}
        </> : <p className="text-sm text-muted">{t("history.empty")}</p>}
      </div>
    </div>
    <p className="border-t border-line px-5 py-3 text-xs text-muted">{fresh[component] ? t("history.refreshed") : t("history.offline")}</p>
  </Card>;
}
