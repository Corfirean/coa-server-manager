import { useEffect, useRef } from "react";
import { Button } from "@/components/ui/button";
import { useI18n } from "@/i18n";

export function ForceStopDialog({ onCancel, onConfirm }: { onCancel: () => void; onConfirm: () => void }) {
  const { t } = useI18n();
  const panel = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const previous = document.activeElement;
    panel.current?.querySelector<HTMLButtonElement>("button")?.focus();
    return () => { if (previous instanceof HTMLElement) previous.focus(); };
  }, []);
  return <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 p-5">
    <div ref={panel} role="alertdialog" aria-modal="true" aria-labelledby="force-stop-title" aria-describedby="force-stop-warning" className="w-full max-w-lg rounded-lg border border-bad/40 bg-card p-6" onKeyDown={e => {
      if (e.key === "Escape") { e.preventDefault(); onCancel(); }
      if (e.key === "Tab") {
        const buttons = panel.current?.querySelectorAll<HTMLButtonElement>("button");
        if (!buttons?.length) return;
        if (e.shiftKey && document.activeElement === buttons[0]) { e.preventDefault(); buttons[buttons.length - 1].focus(); }
        else if (!e.shiftKey && document.activeElement === buttons[buttons.length - 1]) { e.preventDefault(); buttons[0].focus(); }
      }
    }}>
      <h2 id="force-stop-title" className="text-lg font-semibold">{t("stop.forceTitle")}</h2>
      <p id="force-stop-warning" className="mt-3 text-sm text-muted">{t("stop.forceWarning")}</p>
      <div className="mt-5 flex justify-end gap-3"><Button onClick={onCancel}>{t("stop.cancel")}</Button><Button variant="secondary" onClick={onConfirm}>{t("stop.force")}</Button></div>
    </div>
  </div>;
}
