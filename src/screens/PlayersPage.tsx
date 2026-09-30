import { useEffect, useState } from "react";
import { api, asUiError, type UiError } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { useHuman, useT } from "@/i18n";

export function PlayersPage({ serverId }: { serverId: string }) {
  const t = useT();
  const human = useHuman();
  const [online, setOnline] = useState<boolean | null>(null);
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [admin, setAdmin] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<UiError | null>(null);
  const [done, setDone] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    const poll = () => api.status(serverId).then((s) => alive && setOnline(s.observed.world.state === "running")).catch(() => {});
    void poll();
    const t = setInterval(poll, 3000);
    return () => {
      alive = false;
      clearInterval(t);
    };
  }, [serverId]);

  async function create() {
    setBusy(true);
    setError(null);
    setDone(null);
    try {
      await api.createAccount(serverId, username, password, admin);
      setDone(username);
      setUsername("");
      setPassword("");
    } catch (e) {
      setError(asUiError(e));
    } finally {
      setBusy(false);
    }
  }

  const input = "w-72 rounded-md border border-line bg-bg px-3 py-2.5 text-[15px] outline-none focus:border-gold";
  return (
    <div className="max-w-xl">
      <h1 className="text-2xl font-semibold">{t("players.title")}</h1>
      <p className="mt-1 text-muted">{t("q.players")}</p>

      <Card className="mt-6 p-6">
        <h2 className="font-semibold">{t("players.createTitle")}</h2>
        <p className="mt-1 text-sm text-muted">{t("players.createText")}</p>
        {online === false && <p className="mt-3 text-sm text-warn">{t("players.startFirst")}</p>}
        <div className="mt-4 flex flex-col gap-3">
          <div>
            <label htmlFor="acc-user" className="text-sm text-muted">{t("players.username")}</label>
            <input id="acc-user" className={input + " mt-1 block"} value={username} onChange={(e) => setUsername(e.target.value)} autoComplete="off" />
          </div>
          <div>
            <label htmlFor="acc-pass" className="text-sm text-muted">{t("players.password")}</label>
            <input id="acc-pass" type="password" className={input + " mt-1 block"} value={password} onChange={(e) => setPassword(e.target.value)} autoComplete="new-password" />
          </div>
          <label className="flex cursor-pointer items-start gap-2 text-sm">
            <input type="checkbox" checked={admin} onChange={(e) => setAdmin(e.target.checked)} className="mt-1" />
            <span>
              {t("players.admin")}
              <span className="block text-xs text-muted">{t("players.adminHint")}</span>
            </span>
          </label>
        </div>
        <Button className="mt-5" variant="primary" disabled={busy || !online || !username || !password} onClick={() => void create()}>
          {t("players.create")}
        </Button>
        {done && <p className="mt-3 text-sm text-ok" role="status">{t("players.created", { name: done })}</p>}
        {error && (
          <p className="mt-3 text-sm text-bad" role="alert">
            {error.human.code === "unknown" ? error.technical : human(error.human).message}
          </p>
        )}
      </Card>
    </div>
  );
}
