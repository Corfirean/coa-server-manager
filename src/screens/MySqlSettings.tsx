import { useEffect, useState } from "react";
import { api, asUiError, type CustomMysql } from "@/lib/api";
import { Card } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { useT } from "@/i18n";

/** The owner's own MySQL options (#103): kept in Settings/mysql-custom.ini and added to my.ini at every start. */
export function MySqlSettings({ serverId }: { serverId: string }) {
  const t = useT();
  const [saved, setSaved] = useState<CustomMysql | null>(null);
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    void api.mysqlCustom(serverId).then(v => { if (alive) { setSaved(v); setDraft(v.text); } }).catch(e => { if (alive) setError(asUiError(e).technical); });
    return () => { alive = false; };
  }, [serverId]);

  async function save() {
    setBusy(true); setError(null); setDone(false);
    try { const v = await api.mysqlCustomSave(serverId, draft); setSaved(v); setDraft(v.text); setDone(true); }
    catch (e) { setError(asUiError(e).technical); }
    finally { setBusy(false); }
  }

  const applied = saved?.applied.trim() ? saved.applied.trim().split("\n").length : 0;
  return <Card className="mt-6 p-6" aria-busy={busy}>
    <h2 className="font-semibold">{t("mysql.title")}</h2>
    <p className="mt-1 text-sm text-muted">{t("mysql.hint")}</p>
    <textarea
      className="selectable mt-3 h-36 w-full rounded border border-line bg-bg p-3 font-mono text-xs text-ink outline-none focus:border-gold"
      spellCheck={false}
      placeholder={"innodb_buffer_pool_size = 1G\nmax_connections = 300"}
      aria-label={t("mysql.title")}
      value={draft}
      disabled={busy || !saved}
      onChange={e => { setDraft(e.target.value); setDone(false); }}
    />
    <div className="mt-3 flex flex-wrap items-center gap-3">
      <Button size="sm" disabled={busy || !saved || draft === saved.text} onClick={() => void save()}>{t("mysql.save")}</Button>
      <span className="text-xs text-muted">{t("mysql.applied", { count: applied })}{done ? ` · ${t("mysql.saved")}` : ""}</span>
    </div>
    {error && <p className="mt-3 text-sm text-bad" role="alert">{error}</p>}
  </Card>;
}
