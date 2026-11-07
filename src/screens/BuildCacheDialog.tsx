import { useCallback, useEffect, useState } from "react";
import { Loader2, Trash2, X } from "lucide-react";
import { api, asUiError, type BuildCacheSummary } from "@/lib/api";
import { formatBytes } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";

/** What the module build cache takes, with removal of one module, all modules, or the downloaded sources. */
export function BuildCacheDialog(props: { serverId: string; locale: string; onClose: () => void }) {
  const ru = props.locale === "ru";
  const [summary, setSummary] = useState<BuildCacheSummary | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(() => {
    void api
      .buildCacheSummary(props.serverId)
      .then(setSummary)
      .catch((e) => setError(asUiError(e).technical));
  }, [props.serverId]);
  useEffect(load, [load]);

  async function clear(key: string, module: string | null, sources: boolean) {
    setBusy(key);
    setError(null);
    try {
      const freed = await api.buildCacheClear(props.serverId, module, sources);
      setMessage(ru ? `Освобождено ${formatBytes(freed)}` : `Freed ${formatBytes(freed)}`);
      load();
    } catch (e) {
      setError(asUiError(e).technical);
    } finally {
      setBusy(null);
    }
  }

  const when = (secs: number) =>
    secs > 0 ? new Date(secs * 1000).toLocaleDateString(ru ? "ru-RU" : undefined, { day: "numeric", month: "short" }) : "—";

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4" role="dialog" aria-modal aria-label={ru ? "Кэш сборок" : "Build cache"}>
      <Card className="w-full max-w-lg space-y-4 border-line bg-card p-5 shadow-2xl">
        <div className="flex items-start justify-between gap-3">
          <div>
            <h3 className="text-base font-semibold text-ink">{ru ? "Кэш сборок" : "Build cache"}</h3>
            <p className="mt-1 text-xs text-muted">
              {ru
                ? "Скомпилированные части модулей хранятся сжатыми и ускоряют повторную сборку. После обновления ядра старый кэш удаляется сам. Удаление ничего не ломает: следующая сборка модуля будет полной."
                : "Compiled parts of modules are kept compressed and speed up the next build. After a core update the old cache goes by itself. Removing it breaks nothing: the next build of a module is a full one."}
            </p>
          </div>
          <button onClick={props.onClose} className="text-muted hover:text-ink" aria-label={ru ? "Закрыть" : "Close"}>
            <X className="h-4 w-4" />
          </button>
        </div>

        {!summary && !error && <Loader2 className="h-4 w-4 animate-spin text-gold" />}

        {summary && (
          <>
            <div className="divide-y divide-line rounded border border-line text-sm">
              {summary.modules.length === 0 && <p className="p-3 text-xs text-muted">{ru ? "Кэш пуст." : "The cache is empty."}</p>}
              {summary.modules.map((m) => (
                <div key={m.module} className="flex items-center gap-3 px-3 py-2">
                  <div className="min-w-0 flex-1">
                    <p className="truncate font-medium text-ink">{m.module}</p>
                    <p className="text-[11px] text-muted">
                      {m.packed
                        ? ru ? `сжато из ${formatBytes(m.unpacked_bytes)}` : `packed from ${formatBytes(m.unpacked_bytes)}`
                        : ru ? "не сжато" : "not packed"}
                      {" · "}
                      {ru ? "сборка " : "built "}
                      {when(m.last_used)}
                      {!m.current_core && (ru ? " · для другого ядра" : " · for another core")}
                    </p>
                  </div>
                  <span className="text-xs tabular-nums text-muted">{formatBytes(m.bytes)}</span>
                  <Button
                    size="sm"
                    variant="ghost"
                    disabled={busy !== null}
                    onClick={() => void clear(m.module, m.module, false)}
                    aria-label={ru ? `Удалить кэш ${m.module}` : `Remove the cache of ${m.module}`}
                    className="h-7 w-7 p-0 text-muted hover:text-bad"
                  >
                    {busy === m.module ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <Trash2 className="h-3.5 w-3.5" />}
                  </Button>
                </div>
              ))}
            </div>

            <div className="flex items-center justify-between text-xs text-muted">
              <span>
                {ru ? "Скачанные исходники модулей" : "Downloaded module sources"}: {formatBytes(summary.sources_bytes)}
              </span>
              <Button size="sm" variant="ghost" disabled={busy !== null || summary.sources_bytes === 0} onClick={() => void clear("sources", null, true)} className="h-7 text-xs">
                {busy === "sources" ? <Loader2 className="mr-1 h-3.5 w-3.5 animate-spin" /> : null}
                {ru ? "Удалить" : "Remove"}
              </Button>
            </div>

            <div className="flex items-center justify-between border-t border-line pt-3">
              <span className="text-xs text-muted">
                {ru ? "Всего" : "Total"}: {formatBytes(summary.total_bytes)}
                {ru ? `, предел ${formatBytes(summary.limit_bytes)}` : `, limit ${formatBytes(summary.limit_bytes)}`}
              </span>
              <Button size="sm" variant="ghost" disabled={busy !== null || summary.modules.length === 0} onClick={() => void clear("all", null, false)} className="h-7 border border-line text-xs">
                {busy === "all" ? <Loader2 className="mr-1 h-3.5 w-3.5 animate-spin" /> : <Trash2 className="mr-1 h-3.5 w-3.5" />}
                {ru ? "Удалить весь кэш сборок" : "Remove the whole build cache"}
              </Button>
            </div>
          </>
        )}

        {message && <p className="text-xs text-ok" role="status">{message}</p>}
        {error && <p className="text-xs text-bad" role="alert">{error}</p>}
      </Card>
    </div>
  );
}
