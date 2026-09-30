import { useCallback, useEffect, useState } from "react";
import { Check, Loader2, Minus } from "lucide-react";
import { api, asUiError, type FriendsMode, type FriendsStatus, type InternetCheck, type UiError } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";

function Line({ ok, children, warn }: { ok: boolean; warn?: boolean; children: React.ReactNode }) {
  return (
    <li className="flex items-center gap-2 py-1.5 text-sm">
      {ok ? <Check className="h-4 w-4 text-ok" aria-hidden /> : <Minus className={warn ? "h-4 w-4 text-warn" : "h-4 w-4 text-muted"} aria-hidden />}
      <span className={ok ? "" : "text-muted"}>{children}</span>
      <span className="sr-only">{ok ? "OK" : "not yet"}</span>
    </li>
  );
}

const MODES: { id: FriendsMode; title: string; text: string }[] = [
  { id: "lan", title: "Local network", text: "Friends on the same home network or Wi-Fi." },
  { id: "direct", title: "Over the internet", text: "Friends anywhere, through your internet connection. Needs your router to allow it." },
  { id: "private", title: "Private network", text: "Friends anywhere, no router setup. Uses the free Tailscale app." },
];

export function FriendsPage({ serverId }: { serverId: string }) {
  const [st, setSt] = useState<FriendsStatus | null>(null);
  const [net, setNet] = useState<InternetCheck | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<UiError | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  const refresh = useCallback(() => api.friendsStatus(serverId).then(setSt).catch((e) => setError(asUiError(e))), [serverId]);
  useEffect(() => {
    void refresh();
  }, [refresh]);

  async function run(label: string, fn: () => Promise<string | void>) {
    setBusy(label);
    setError(null);
    setNote(null);
    try {
      const msg = await fn();
      if (msg) setNote(msg);
      await refresh();
    } catch (e) {
      setError(asUiError(e));
    } finally {
      setBusy(null);
    }
  }

  const check = () =>
    run("check", async () => {
      setNet(await api.friendsCheckInternet());
    });

  const enable = (mode: FriendsMode) =>
    run(`enable-${mode}`, async () => {
      const r = await api.friendsEnable(serverId, mode, net?.public_ip ?? undefined, mode === "direct" && !!net?.router_found);
      return [r.restart_required ? "Restart the server to apply this." : null, r.note].filter(Boolean).join(" ") || "Ready for friends.";
    });

  if (!st) return <p className="text-muted">{error ? error.human.message : "Looking at your connection…"}</p>;

  const cur = st.settings;
  const exposed = st.exposure.filter((e) => (e.what === "database" || e.what === "server console") && e.reachable_from_network);
  const connection = cur.mode === "local" ? null : `set realmlist ${cur.host}`;

  return (
    <div className="max-w-3xl">
      <h1 className="text-2xl font-semibold">Play with Friends</h1>
      <p className="mt-1 text-muted">How can my friend join?</p>

      {exposed.length > 0 && (
        <Card className="mt-5 border-bad/40 p-4" role="alert">
          <p className="font-medium text-bad">Something private is reachable from your network</p>
          <p className="mt-1 text-sm text-muted">{exposed.map((e) => e.what).join(" and ")} accept connections from other computers. Restart the server from this program to close them.</p>
        </Card>
      )}

      <Card className="mt-6 p-6">
        <h2 className="font-semibold">Status</h2>
        <ul className="mt-2 divide-y divide-line">
          <Line ok={st.server_running}>Server is running</Line>
          <Line ok={st.servers_open}>Login and game servers are open to other computers</Line>
          <Line ok={st.firewall.auth && st.firewall.world}>Windows Firewall allows the game ports</Line>
          <Line ok={st.exposure.filter((e) => e.what === "database" || e.what === "server console").every((e) => !e.reachable_from_network)}>
            Database and server console are private to this computer
          </Line>
        </ul>
      </Card>

      <div className="mt-6 grid gap-3">
        {MODES.map((m) => (
          <Card key={m.id} className={`p-5 ${cur.mode === m.id ? "border-gold/60" : ""}`}>
            <div className="flex items-start justify-between gap-4">
              <div>
                <h3 className="font-semibold">
                  {m.title} {cur.mode === m.id && <span className="ml-2 rounded bg-gold/15 px-1.5 py-0.5 text-xs font-normal text-gold">In use</span>}
                </h3>
                <p className="mt-0.5 text-sm text-muted">{m.text}</p>

                {m.id === "lan" && <p className="mt-2 text-sm">Your address on this network: <b className="selectable">{st.lan_ip ?? "unknown"}</b></p>}

                {m.id === "direct" && (
                  <div className="mt-2 text-sm">
                    <Button size="sm" variant="ghost" disabled={!!busy} onClick={() => void check()}>
                      {busy === "check" && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
                      Check my connection
                    </Button>
                    {net && net.reachability === "cgnat" && (
                      <p className="mt-2 text-warn">
                        Your internet provider does not allow direct hosting. Use the private network instead.
                      </p>
                    )}
                    {net?.public_ip && net.reachability !== "cgnat" && (
                      <p className="mt-2">Your public address: <b className="selectable">{net.public_ip}</b>{!net.router_found && <span className="text-muted"> · your router cannot be set up automatically; forward the two game ports by hand</span>}</p>
                    )}
                    {net && !net.public_ip && <p className="mt-2 text-warn">Could not reach the internet to find your address.</p>}
                  </div>
                )}

                {m.id === "private" && (
                  <p className="mt-2 text-sm">
                    {!st.tailscale.installed && <span className="text-muted">Tailscale is not installed. Install the free app from tailscale.com/download, sign in, then come back.</span>}
                    {st.tailscale.installed && !st.tailscale.connected && <span className="text-warn">Tailscale is installed but not connected. Sign in to it first.</span>}
                    {st.tailscale.connected && <>Your private address: <b className="selectable">{st.tailscale.ip}</b></>}
                  </p>
                )}
              </div>
              <Button
                size="sm"
                variant={m.id === "private" && net?.reachability === "cgnat" ? "primary" : "secondary"}
                disabled={!!busy || (m.id === "direct" && !net?.public_ip) || (m.id === "private" && !st.tailscale.connected)}
                onClick={() => void enable(m.id)}
                title={m.id === "direct" && !net?.public_ip ? "Check your connection first" : undefined}
              >
                {busy === `enable-${m.id}` && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
                {cur.mode === m.id ? "Re-apply" : "Enable"}
              </Button>
            </div>
          </Card>
        ))}
        {cur.mode !== "local" && (
          <Button variant="ghost" size="sm" className="justify-self-start" disabled={!!busy} onClick={() => void run("local", async () => { await api.friendsEnable(serverId, "local", undefined, false); return "Only this computer can connect now."; })}>
            Stop sharing (this computer only)
          </Button>
        )}
      </div>

      {connection && (
        <Card className="mt-6 p-6">
          <h2 className="font-semibold">Give this to your friend</h2>
          <p className="mt-1 text-sm text-muted">They put this line in their game's realmlist.wtf file. Passwords and settings are never shared.</p>
          <pre className="selectable mt-3 rounded bg-black/40 p-3 text-sm">{connection}</pre>
          <div className="mt-3 flex flex-wrap gap-2">
            <Button size="sm" onClick={() => void navigator.clipboard.writeText(connection).then(() => { setCopied(true); setTimeout(() => setCopied(false), 2000); })}>
              {copied ? "Copied" : "Copy connection info"}
            </Button>
            <Button size="sm" variant="ghost" disabled={!!busy} onClick={() => void run("pkg", async () => `Saved: ${await api.friendsPackage(serverId)}`)}>
              Create friend package (zip)
            </Button>
          </div>
        </Card>
      )}

      {note && <p className="mt-4 text-sm text-ok" role="status">{note}</p>}
      {error && <p className="mt-4 text-sm text-bad" role="alert">{error.human.code === "unknown" ? error.technical : error.human.message}</p>}
    </div>
  );
}
