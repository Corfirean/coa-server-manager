import { useEffect, useState } from "react";
import { api, asUiError, type RestartSchedule, type Weekday } from "@/lib/api";
import { Card } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { useT } from "@/i18n";

const WEEKDAYS: Weekday[] = ["monday", "tuesday", "wednesday", "thursday", "friday", "saturday", "sunday"];
const DEFAULT_SCHEDULE: RestartSchedule = {
  enabled: false,
  time: "05:00",
  warningMinutes: 5,
  weeklyEnabled: false,
  weeklyDay: "sunday",
  weeklyTime: "05:00",
  weeklyWarningMinutes: 5,
};

/** Daily and weekly world restarts at chosen local times, with an in-game countdown (#202). */
export function RestartScheduleSettings({ serverId }: { serverId: string }) {
  const t = useT();
  const [saved, setSaved] = useState<RestartSchedule | null>(null);
  const [draft, setDraft] = useState<RestartSchedule>(DEFAULT_SCHEDULE);
  const [supported, setSupported] = useState(true);
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    void api.restartSchedule(serverId).then(v => { if (alive) { setSaved(v.schedule); setDraft(v.schedule); setSupported(v.supported); } }).catch(e => { if (alive) setError(asUiError(e).technical); });
    return () => { alive = false; };
  }, [serverId]);

  async function save() {
    setBusy(true); setError(null); setDone(false);
    try { const v = await api.restartScheduleSave(serverId, draft); setSaved(v.schedule); setDraft(v.schedule); setSupported(v.supported); setDone(true); }
    catch (e) { setError(asUiError(e).technical); }
    finally { setBusy(false); }
  }

  const changed = !!saved && JSON.stringify(saved) !== JSON.stringify(draft);
  const minutesOk = Number.isInteger(draft.warningMinutes) && draft.warningMinutes >= 1 && draft.warningMinutes <= 60;
  const weeklyMinutesOk = Number.isInteger(draft.weeklyWarningMinutes) && draft.weeklyWarningMinutes >= 1 && draft.weeklyWarningMinutes <= 60;
  const valid = (!draft.enabled || minutesOk) && (!draft.weeklyEnabled || weeklyMinutesOk);
  const input = "rounded border border-line bg-bg px-2 py-1 text-sm text-ink outline-none focus:border-gold";
  return <Card className="mt-6 p-6" aria-busy={busy}>
    <h2 className="font-semibold">{t("sched.title")}</h2>
    <label className="mt-3 flex cursor-pointer items-center gap-3">
      <input type="checkbox" className="h-4 w-4 accent-[#c9a24a]" checked={draft.enabled} disabled={busy || !saved} onChange={e => { setDraft(d => ({ ...d, enabled: e.target.checked })); setDone(false); }} />
      <span className="text-sm font-medium">{t("sched.enable")}</span>
    </label>
    <div className="mt-3 flex flex-wrap items-center gap-4 text-sm">
      <label className="flex items-center gap-2">
        <span>{t("sched.time")}</span>
        <input type="time" className={input} value={draft.time} disabled={busy || !draft.enabled} onChange={e => { setDraft(d => ({ ...d, time: e.target.value })); setDone(false); }} />
      </label>
      <label className="flex items-center gap-2">
        <span>{t("sched.warning")}</span>
        <input type="number" min={1} max={60} step={1} className={`${input} w-20`} value={draft.warningMinutes} disabled={busy || !draft.enabled} onChange={e => { setDraft(d => ({ ...d, warningMinutes: Number(e.target.value) })); setDone(false); }} />
      </label>
    </div>

    <div className="mt-4 border-t border-line pt-4">
      <label className="flex cursor-pointer items-center gap-3">
        <input type="checkbox" className="h-4 w-4 accent-[#c9a24a]" checked={draft.weeklyEnabled} disabled={busy || !saved} onChange={e => { setDraft(d => ({ ...d, weeklyEnabled: e.target.checked })); setDone(false); }} />
        <span className="text-sm font-medium">{t("sched.weeklyEnable")}</span>
      </label>
      <div className="mt-3 flex flex-wrap items-center gap-4 text-sm">
        <label className="flex items-center gap-2">
          <span>{t("sched.day")}</span>
          <select className={input} value={draft.weeklyDay} disabled={busy || !draft.weeklyEnabled} onChange={e => { setDraft(d => ({ ...d, weeklyDay: e.target.value as Weekday })); setDone(false); }}>
            {WEEKDAYS.map(day => <option key={day} value={day}>{t(`sched.days.${day}`)}</option>)}
          </select>
        </label>
        <label className="flex items-center gap-2">
          <span>{t("sched.time")}</span>
          <input type="time" className={input} value={draft.weeklyTime} disabled={busy || !draft.weeklyEnabled} onChange={e => { setDraft(d => ({ ...d, weeklyTime: e.target.value })); setDone(false); }} />
        </label>
        <label className="flex items-center gap-2">
          <span>{t("sched.warning")}</span>
          <input type="number" min={1} max={60} step={1} className={`${input} w-20`} value={draft.weeklyWarningMinutes} disabled={busy || !draft.weeklyEnabled} onChange={e => { setDraft(d => ({ ...d, weeklyWarningMinutes: Number(e.target.value) })); setDone(false); }} />
        </label>
      </div>
    </div>

    <p className="mt-3 text-sm text-muted">{t("sched.hint")}</p>
    {!supported && <p className="mt-2 text-sm text-warn">{t("sched.unsupported")}</p>}
    <div className="mt-3 flex items-center gap-3">
      <Button size="sm" disabled={busy || !changed || !valid} onClick={() => void save()}>{t("sched.save")}</Button>
      {done && <span className="text-xs text-muted">{t("sched.saved")}</span>}
    </div>
    {error && <p className="mt-3 text-sm text-bad" role="alert">{error}</p>}
  </Card>;
}
