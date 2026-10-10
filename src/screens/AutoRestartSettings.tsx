import { Card } from "@/components/ui/card";
import { useT } from "@/i18n";
import { useAutoRestart } from "@/lib/autoRestartPrefs";

export function AutoRestartSettings({ serverId }: { serverId: string }) {
  const t = useT();
  const [autoRestart, setAutoRestart] = useAutoRestart(serverId);

  return (
    <Card className="mt-6 p-6">
      <h2 className="font-semibold">{t("crash.autoRestart")}</h2>
      <label className="mt-3 flex cursor-pointer items-center gap-3">
        <input
          type="checkbox"
          checked={autoRestart}
          onChange={(e) => setAutoRestart(e.target.checked)}
          className="h-4 w-4 accent-[#c9a24a]"
        />
        <span className="text-sm font-medium">{t("crash.autoRestart")}</span>
      </label>
      <p className="mt-2 text-sm text-muted">{t("crash.autoRestartDesc")}</p>
    </Card>
  );
}
