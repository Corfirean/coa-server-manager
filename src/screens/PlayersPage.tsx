import { useEffect, useState } from "react";
import { api, asUiError, type UiError } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";

export function PlayersPage({ serverId }: { serverId: string }) {
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
      <h1 className="text-2xl font-semibold">Players</h1>
      <p className="mt-1 text-muted">Who is playing right now?</p>

      <Card className="mt-6 p-6">
        <h2 className="font-semibold">Create your account</h2>
        <p className="mt-1 text-sm text-muted">This is the name and password you use to log in to the game.</p>
        {online === false && <p className="mt-3 text-sm text-warn">Start the server first — accounts are created while it is running.</p>}
        <div className="mt-4 flex flex-col gap-3">
          <div>
            <label htmlFor="acc-user" className="text-sm text-muted">Username</label>
            <input id="acc-user" className={input + " mt-1 block"} value={username} onChange={(e) => setUsername(e.target.value)} autoComplete="off" />
          </div>
          <div>
            <label htmlFor="acc-pass" className="text-sm text-muted">Password</label>
            <input id="acc-pass" type="password" className={input + " mt-1 block"} value={password} onChange={(e) => setPassword(e.target.value)} autoComplete="new-password" />
          </div>
          <label className="flex cursor-pointer items-start gap-2 text-sm">
            <input type="checkbox" checked={admin} onChange={(e) => setAdmin(e.target.checked)} className="mt-1" />
            <span>
              Make this account an administrator
              <span className="block text-xs text-muted">Administrators can use game-master commands in the game. Only do this for accounts you trust.</span>
            </span>
          </label>
        </div>
        <Button className="mt-5" variant="primary" disabled={busy || !online || !username || !password} onClick={() => void create()}>
          Create account
        </Button>
        {done && <p className="mt-3 text-sm text-ok" role="status">Account “{done}” created. You can log in to the game with it now.</p>}
        {error && (
          <p className="mt-3 text-sm text-bad" role="alert">
            {error.human.code === "unknown" ? error.technical : error.human.message}
          </p>
        )}
      </Card>
    </div>
  );
}
