import { useEffect, useState } from "react";
import { AlertTriangle, Check, ChevronDown, ChevronUp, Copy, Loader2, X } from "lucide-react";
import { api, type CrashItem } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { useT } from "@/i18n";

export function CrashHistoryModal({
  serverId,
  onClose,
}: {
  serverId: string;
  onClose: () => void;
}) {
  const t = useT();
  const [loading, setLoading] = useState(true);
  const [crashes, setCrashes] = useState<CrashItem[]>([]);
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [copiedId, setCopiedId] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    void api
      .listCrashes(serverId, 30)
      .then((items) => {
        if (alive) {
          setCrashes(items);
          setLoading(false);
        }
      })
      .catch(() => {
        if (alive) setLoading(false);
      });
    return () => {
      alive = false;
    };
  }, [serverId]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const copyReport = async (item: CrashItem) => {
    try {
      await navigator.clipboard.writeText(item.full_log);
      setCopiedId(item.id);
      setTimeout(() => setCopiedId(null), 2500);
    } catch {
      /* clipboard write failed */
    }
  };

  const getCategoryColor = (category: CrashItem["category"]) => {
    switch (category) {
      case "assertion":
        return "bg-amber-500/15 text-amber-400 border-amber-500/30";
      case "access_violation":
      case "stack_overflow":
      case "out_of_memory":
        return "bg-bad/15 text-bad border-bad/30";
      case "network":
      case "database":
        return "bg-blue-500/15 text-blue-400 border-blue-500/30";
      default:
        return "bg-muted/15 text-muted border-line";
    }
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4 md:p-6"
      role="presentation"
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <Card
        className="flex max-h-[85vh] w-full max-w-2xl flex-col p-6 shadow-2xl"
        role="dialog"
        aria-modal="true"
        aria-label={t("crash.historyTitle")}
      >
        <div className="flex items-center justify-between border-b border-line pb-4">
          <div className="flex items-center gap-2.5">
            <AlertTriangle className="h-5 w-5 text-warn" aria-hidden />
            <h2 className="text-lg font-semibold">{t("crash.historyTitle")}</h2>
          </div>
          <button
            type="button"
            onClick={onClose}
            className="cursor-pointer text-muted transition-colors hover:text-ink"
            aria-label={t("btn.close")}
          >
            <X className="h-5 w-5" aria-hidden />
          </button>
        </div>

        <div className="flex-1 overflow-y-auto py-4">
          {loading ? (
            <div className="flex items-center justify-center py-12 text-muted">
              <Loader2 className="h-6 w-6 animate-spin" aria-hidden />
            </div>
          ) : crashes.length === 0 ? (
            <div className="py-12 text-center text-muted">
              <p>{t("crash.noCrashes")}</p>
            </div>
          ) : (
            <div className="space-y-4">
              {crashes.map((item) => {
                const isExpanded = expandedId === item.id;
                const isCopied = copiedId === item.id;
                return (
                  <div
                    key={item.id}
                    className="rounded-lg border border-line bg-card-2 p-4 text-sm transition-all"
                  >
                    <div className="flex flex-wrap items-start justify-between gap-2">
                      <div>
                        <span
                          className={`inline-block rounded border px-2 py-0.5 text-xs font-medium uppercase tracking-wide ${getCategoryColor(
                            item.category,
                          )}`}
                        >
                          {item.exception_code ? `${item.category} (${item.exception_code})` : item.category}
                        </span>
                        <h3 className="mt-1.5 font-semibold text-ink">{item.title}</h3>
                      </div>
                      <span className="text-xs text-muted tabular-nums">{item.date_str}</span>
                    </div>

                    <p className="mt-2 text-xs leading-relaxed text-muted">{item.explanation}</p>

                    <div className="mt-3 flex items-center justify-between border-t border-line/50 pt-2.5">
                      <button
                        type="button"
                        onClick={() => setExpandedId(isExpanded ? null : item.id)}
                        className="inline-flex cursor-pointer items-center gap-1.5 text-xs font-medium text-gold transition-colors hover:underline"
                      >
                        {isExpanded ? (
                          <>
                            <ChevronUp className="h-3.5 w-3.5" aria-hidden />
                            {t("crash.hideRaw")}
                          </>
                        ) : (
                          <>
                            <ChevronDown className="h-3.5 w-3.5" aria-hidden />
                            {t("crash.viewRaw")}
                          </>
                        )}
                      </button>

                      <Button
                        variant="ghost"
                        size="sm"
                        onClick={() => void copyReport(item)}
                        className="h-7 gap-1.5 px-2 text-xs text-muted hover:text-ink"
                      >
                        {isCopied ? (
                          <>
                            <Check className="h-3.5 w-3.5 text-ok" aria-hidden />
                            <span className="text-ok">{t("crash.copied")}</span>
                          </>
                        ) : (
                          <>
                            <Copy className="h-3.5 w-3.5" aria-hidden />
                            <span>{t("crash.copyReport")}</span>
                          </>
                        )}
                      </Button>
                    </div>

                    {isExpanded && (
                      <div className="mt-3">
                        <pre className="selectable max-h-56 overflow-x-auto overflow-y-auto rounded border border-line bg-black/40 p-3 font-mono text-[11px] leading-relaxed text-muted">
                          {item.full_log}
                        </pre>
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          )}
        </div>

        <div className="flex justify-end border-t border-line pt-4">
          <Button variant="secondary" onClick={onClose}>
            {t("btn.close")}
          </Button>
        </div>
      </Card>
    </div>
  );
}
