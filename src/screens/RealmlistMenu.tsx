import { useCallback, useEffect, useRef, useState } from "react";
import { Check, ChevronDown, Pencil, Plus, Trash2 } from "lucide-react";
import { api, asUiError, type RealmProfile, type UiError } from "@/lib/api";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { useHuman, useT } from "@/i18n";

/** The first `set realmlist` value of a realmlist text, for the small caption under a name. */
function host(data: string): string {
  const m = data.match(/^\s*set\s+realmlist\s+(\S+)/im);
  return m ? m[1].replace(/"/g, "") : "";
}

/** Named realmlists for the game client, switched with one click: "Solo" for your own server, "PTR" for the shared one, and so on. */
export function RealmlistMenu({ serverId, className }: { serverId: string; className?: string }) {
  const t = useT();
  const human = useHuman();
  const [profiles, setProfiles] = useState<RealmProfile[]>([]);
  const [active, setActive] = useState<string | null>(null);
  const [open, setOpen] = useState(false);
  const [editing, setEditing] = useState<{ id: string | null; name: string; data: string } | null>(null);
  const [error, setError] = useState<UiError | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const box = useRef<HTMLDivElement>(null);

  const load = useCallback(async () => {
    try {
      const v = await api.realmlistProfiles(serverId);
      setProfiles(v.profiles);
      setActive(v.active);
    } catch (e) {
      setError(asUiError(e));
    }
  }, [serverId]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    if (!open) return;
    const away = (e: MouseEvent) => {
      if (box.current && !box.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", away);
    return () => document.removeEventListener("mousedown", away);
  }, [open]);

  async function choose(p: RealmProfile) {
    setError(null);
    setNote(null);
    try {
      await api.realmlistActivate(serverId, p.id);
      setActive(p.id);
      setNote(t("realm.switched", { name: p.name }));
    } catch (e) {
      setError(asUiError(e));
    }
  }

  async function remove(p: RealmProfile) {
    if (!window.confirm(t("realm.confirmDelete", { name: p.name }))) return;
    setError(null);
    try {
      await api.realmlistDelete(serverId, p.id);
      await load();
    } catch (e) {
      setError(asUiError(e));
    }
  }

  async function save() {
    if (!editing) return;
    setError(null);
    try {
      await api.realmlistSave(serverId, editing.id, editing.name, editing.data);
      setEditing(null);
      await load();
    } catch (e) {
      setError(asUiError(e));
    }
  }

  const current = profiles.find((p) => p.id === active);
  const message = error ? (error.human.code === "unknown" ? error.technical : human(error.human).message) : null;

  return (
    <div ref={box} className={cn("relative", className)}>
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-haspopup="menu"
        aria-expanded={open}
        className="flex h-16 cursor-pointer items-center gap-3 rounded-md border border-line bg-card-2 px-4 text-left transition-colors hover:border-gold/50"
      >
        <span>
          <span className="block text-[11px] uppercase tracking-wide text-muted">{t("realm.label")}</span>
          <span className="block max-w-40 truncate text-[15px] font-medium">{current ? current.name : t("realm.other")}</span>
        </span>
        <ChevronDown className="h-4 w-4 text-muted" aria-hidden />
      </button>

      {open && (
        <div role="menu" className="absolute right-0 top-full z-30 mt-2 w-80 rounded-md border border-line bg-card shadow-2xl">
          <ul className="max-h-72 overflow-y-auto py-1">
            {profiles.map((p) => (
              <li key={p.id} className="group flex items-center">
                <button role="menuitemradio" aria-checked={p.id === active} onClick={() => void choose(p)} className="flex min-w-0 flex-1 cursor-pointer items-center gap-3 px-3 py-2 text-left hover:bg-white/5">
                  <span className="flex h-4 w-4 shrink-0 items-center justify-center text-gold">{p.id === active && <Check className="h-4 w-4" aria-hidden />}</span>
                  <span className="min-w-0">
                    <span className="block truncate text-sm">{p.name}</span>
                    <span className="selectable block truncate text-xs text-muted">{host(p.data)}</span>
                  </span>
                </button>
                <button aria-label={t("realm.edit", { name: p.name })} title={t("realm.edit", { name: p.name })} onClick={() => setEditing({ id: p.id, name: p.name, data: p.data.trim() })} className="cursor-pointer p-2 text-muted hover:text-ink">
                  <Pencil className="h-3.5 w-3.5" aria-hidden />
                </button>
                <button aria-label={t("realm.delete", { name: p.name })} title={t("realm.delete", { name: p.name })} disabled={profiles.length <= 1} onClick={() => void remove(p)} className="cursor-pointer p-2 text-muted hover:text-bad disabled:cursor-default disabled:opacity-30">
                  <Trash2 className="h-3.5 w-3.5" aria-hidden />
                </button>
              </li>
            ))}
          </ul>
          <div className="border-t border-line p-1">
            <button role="menuitem" onClick={() => { setEditing({ id: null, name: "", data: "" }); setError(null); }} className="flex w-full cursor-pointer items-center gap-2 rounded px-3 py-2 text-sm hover:bg-white/5">
              <Plus className="h-4 w-4 text-gold" aria-hidden /> {t("realm.add")}
            </button>
          </div>
          {note && <p className="border-t border-line px-3 py-2 text-xs text-ok" role="status">{note}</p>}
          {message && !editing && <p className="border-t border-line px-3 py-2 text-xs text-bad" role="alert">{message}</p>}
        </div>
      )}

      {editing && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-6" role="presentation">
          <Card className="w-full max-w-md p-6 shadow-2xl" role="dialog" aria-modal="true" aria-label={editing.id ? t("realm.editTitle") : t("realm.addTitle")}>
            <h2 className="text-lg font-semibold">{editing.id ? t("realm.editTitle") : t("realm.addTitle")}</h2>
            <label htmlFor="realm-name" className="mt-4 block text-sm text-muted">{t("realm.name")}</label>
            <input id="realm-name" autoFocus maxLength={32} value={editing.name} onChange={(e) => setEditing({ ...editing, name: e.target.value })} placeholder={t("realm.nameHint")} className="mt-1 block w-full rounded-md border border-line bg-bg px-3 py-2.5 text-[15px] outline-none focus:border-gold" />
            <label htmlFor="realm-data" className="mt-4 block text-sm text-muted">{t("realm.data")}</label>
            <textarea id="realm-data" rows={3} value={editing.data} onChange={(e) => setEditing({ ...editing, data: e.target.value })} placeholder="set realmlist play.example.com" className="selectable mt-1 block w-full rounded-md border border-line bg-bg px-3 py-2.5 font-mono text-sm outline-none focus:border-gold" />
            <p className="mt-1 text-xs text-muted">{t("realm.dataHint")}</p>
            {message && <p className="mt-3 text-sm text-bad" role="alert">{message}</p>}
            <div className="mt-5 flex justify-end gap-2">
              <Button variant="ghost" onClick={() => { setEditing(null); setError(null); }}>{t("common.cancel")}</Button>
              <Button variant="primary" disabled={!editing.name.trim() || !editing.data.trim()} onClick={() => void save()}>{t("acc.save")}</Button>
            </div>
          </Card>
        </div>
      )}
    </div>
  );
}
