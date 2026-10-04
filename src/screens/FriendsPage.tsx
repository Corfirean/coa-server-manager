import { useCallback, useEffect, useState } from "react";
import { Check, Loader2, Minus } from "lucide-react";
import { api, asUiError, type FriendsMode, type FriendsStatus, type InternetCheck, type UiError } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { useHuman, useT, type Key } from "@/i18n";

/** Convenience only; Rust validates again before applying or saving. */
function validLanIpv4(value: string): boolean {
  if (!/^(0|[1-9]\d{0,2})(\.(0|[1-9]\d{0,2})){3}$/.test(value)) return false;
  const octets = value.split(".").map(Number);
  const [a, b] = octets;
  return octets.every((n) => n <= 255) && a > 0 && a < 224 && a !== 127
    && !(a === 169 && b === 254) && !(a === 100 && b >= 64 && b <= 127);
}

function Line({ ok, children, warn }: { ok: boolean; warn?: boolean; children: React.ReactNode }) {
  const t = useT();
  return (
    <li className="flex items-center gap-2 py-1.5 text-sm">
      {ok ? <Check className="h-4 w-4 text-ok" aria-hidden /> : <Minus className={warn ? "h-4 w-4 text-warn" : "h-4 w-4 text-muted"} aria-hidden />}
      <span className={ok ? "" : "text-muted"}>{children}</span>
      <span className="sr-only">{ok ? t("fr.ok") : t("fr.notYet")}</span>
    </li>
  );
}

const MODES: { id: FriendsMode; title: Key; text: Key }[] = [
  { id: "lan", title: "fr.mode.lan.title", text: "fr.mode.lan.text" },
  { id: "direct", title: "fr.mode.direct.title", text: "fr.mode.direct.text" },
  { id: "private", title: "fr.mode.private.title", text: "fr.mode.private.text" },
];

/** A link to one of the few help pages the Manager may open; it goes through the backend, which checks the address. */
function ExtLink({ url, children }: { url: string; children: React.ReactNode }) {
  return (
    <button type="button" onClick={() => void api.openLink(url).catch(() => {})} className="cursor-pointer text-left text-gold underline underline-offset-2 hover:text-gold-strong">
      {children}
    </button>
  );
}

/** Collapsible step-by-step help inside a mode card. */
function Guide({ steps, links }: { steps: string[]; links?: React.ReactNode }) {
  const t = useT();
  return (
    <details className="mt-3 text-sm">
      <summary className="cursor-pointer text-muted hover:text-ink">{t("fr.guide")}</summary>
      <ol className="mt-2 list-decimal space-y-1.5 pl-5 text-muted">
        {steps.map((s, i) => (
          <li key={i}>{s}</li>
        ))}
      </ol>
      {links && <div className="mt-2 flex flex-col items-start gap-1">{links}</div>}
    </details>
  );
}

/** A ready-to-paste realmlist line for the friend, shown as soon as the address is known. */
function RealmlistLine({ host }: { host: string }) {
  const t = useT();
  const [copied, setCopied] = useState(false);
  const line = `set realmlist ${host}`;
  return (
    <div className="mt-2 flex flex-wrap items-center gap-2">
      <code className="selectable rounded bg-black/40 px-2 py-1 text-sm">{line}</code>
      <Button
        size="sm"
        variant="ghost"
        onClick={() =>
          void navigator.clipboard.writeText(line).then(() => {
            setCopied(true);
            setTimeout(() => setCopied(false), 2000);
          })
        }
      >
        {copied ? t("fr.copied") : t("fr.copyLine")}
      </Button>
    </div>
  );
}

export function FriendsPage({ serverId }: { serverId: string }) {
  const t = useT();
  const human = useHuman();
  const [st, setSt] = useState<FriendsStatus | null>(null);
  const [net, setNet] = useState<InternetCheck | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<UiError | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [needsRestart, setNeedsRestart] = useState(false);
  const [lanChoice, setLanChoice] = useState("automatic");
  const [customLan, setCustomLan] = useState("");

  const refresh = useCallback(() => api.friendsStatus(serverId).then((status) => {
    setSt(status);
  }).catch((e) => setError(asUiError(e))), [serverId]);
  useEffect(() => {
    let cancelled = false;
    setSt(null);
    setNet(null);
    void api.friendsStatus(serverId).then((status) => {
      if (cancelled) return;
      const saved = status.settings.lan_address_override;
      setCustomLan(saved ?? "");
      setLanChoice(saved === null ? "automatic" : status.lan_addresses.some((a) => a.address === saved) ? saved : "custom");
      setSt(status);
    }).catch((e) => { if (!cancelled) setError(asUiError(e)); });
    return () => { cancelled = true; };
  }, [serverId]);

  const lanOverride = lanChoice === "automatic" ? null : lanChoice === "custom" ? customLan.trim() : lanChoice;
  const lanValid = lanOverride === null || validLanIpv4(lanOverride);

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
      const r = await api.friendsEnable(
        serverId, mode,
        mode === "direct" ? net?.public_ip ?? undefined : undefined,
        mode === "direct" && !!net?.router_found,
        mode === "lan" ? lanOverride : null,
      );
      setNeedsRestart(r.restart_required);
      return [r.restart_required ? t("fr.restartToApply") : null, r.note ? (r.note.startsWith("Your router was asked") ? t("fr.note.forwarded") : r.note.startsWith("Your router does not support") ? t("fr.note.manual") : r.note) : null].filter(Boolean).join(" ") || t("fr.ready");
    });

  if (!st) return <p className="text-muted">{error ? human(error.human).message : t("fr.looking")}</p>;

  const cur = st.settings;
  const exposed = st.exposure.filter((e) => (e.what === "database" || e.what === "server console") && e.reachable_from_network);
  const connection = cur.mode === "local" ? null : `set realmlist ${cur.host}`;
  const selectedLan = lanOverride ?? st.lan_ip;
  const lanUnavailable = lanOverride !== null && lanValid && !st.lan_addresses.some((a) => a.address === lanOverride);

  return (
    <div className="max-w-3xl">
      <h1 className="text-2xl font-semibold">{t("fr.title")}</h1>
      <p className="mt-1 text-muted">{t("q.friends")}</p>

      {exposed.length > 0 && (
        <Card className="mt-5 border-bad/40 p-4" role="alert">
          <p className="font-medium text-bad">{t("fr.exposedTitle")}</p>
          <p className="mt-1 text-sm text-muted">{t("fr.exposedText", { what: exposed.map((e) => t(e.what === "database" ? "fr.what.database" : "fr.what.console")).join(t("fr.and")) })}</p>
        </Card>
      )}

      <Card className="mt-6 p-6">
        <h2 className="font-semibold">{t("fr.status")}</h2>
        <ul className="mt-2 divide-y divide-line">
          <Line ok={st.server_running}>{t("fr.st.running")}</Line>
          <Line ok={st.servers_open}>{t("fr.st.open")}</Line>
          {st.firewall && <Line ok={st.firewall.auth && st.firewall.world}>{t("fr.st.firewall")}</Line>}
          <Line ok={st.exposure.filter((e) => e.what === "database" || e.what === "server console").every((e) => !e.reachable_from_network)}>
            {t("fr.st.private")}
          </Line>
        </ul>
        {st.secondary_world_port && <p className="mt-3 text-sm text-muted">{t("realm.secondPort", { port: st.secondary_world_port })}</p>}
      </Card>

      <div className="mt-6 grid gap-3">
        {MODES.map((m) => (
          <Card key={m.id} className={`p-5 ${cur.mode === m.id ? "border-gold/60" : ""}`}>
            <div className="flex items-start justify-between gap-4">
              <div>
                <h3 className="font-semibold">
                  {t(m.title)} {cur.mode === m.id && <span className="ml-2 rounded bg-gold/15 px-1.5 py-0.5 text-xs font-normal text-gold">{t("fr.inUse")}</span>}
                </h3>
                <p className="mt-0.5 text-sm text-muted">{t(m.text)}</p>

                {m.id === "lan" && (
                  <>
                    <label htmlFor="lan-address" className="mt-3 block text-sm">{t("fr.lan.address")}</label>
                    <select id="lan-address" value={lanChoice} disabled={!!busy}
                      className="mt-1 w-full rounded-md border border-line bg-bg px-3 py-2 text-sm outline-none focus:border-gold"
                      onChange={(e) => {
                        if (e.target.value === "custom" && lanOverride) setCustomLan(lanOverride);
                        setLanChoice(e.target.value);
                      }}>
                      <option value="automatic">{t("fr.lan.automatic")} — {st.lan_ip ?? t("fr.unknown")}</option>
                      {st.lan_addresses.map((a) => <option key={a.address} value={a.address}>{a.interface} — {a.address}{a.is_default ? ` (${t("fr.lan.default")})` : ""}</option>)}
                      {lanChoice !== "automatic" && lanChoice !== "custom" && !st.lan_addresses.some((a) => a.address === lanChoice) && <option value={lanChoice}>{lanChoice}</option>}
                      <option value="custom">{t("fr.lan.custom")}</option>
                    </select>
                    {lanChoice === "custom" && <>
                      <label htmlFor="lan-custom" className="mt-2 block text-sm">{t("fr.lan.customIpv4")}</label>
                      <input id="lan-custom" value={customLan} disabled={!!busy} spellCheck={false} inputMode="decimal"
                        aria-invalid={!lanValid} aria-describedby={!lanValid ? "lan-invalid" : undefined}
                        className="mt-1 w-full rounded-md border border-line bg-bg px-3 py-2 text-sm outline-none focus:border-gold"
                        onChange={(e) => setCustomLan(e.target.value)} />
                    </>}
                    {!lanValid && <p id="lan-invalid" className="mt-2 text-sm text-bad" role="alert">{t("fr.lan.invalid")}</p>}
                    {lanUnavailable && <p className="mt-2 text-sm text-warn" role="status"><b>{t("fr.lan.unavailable")}</b> {t("fr.lan.unavailableText")}</p>}
                    {selectedLan && lanValid && <RealmlistLine host={selectedLan} />}
                  </>
                )}

                {m.id === "direct" && (
                  <div className="mt-2 text-sm">
                    <Button size="sm" variant="ghost" disabled={!!busy} onClick={() => void check()}>
                      {busy === "check" && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
                      {t("fr.checkConn")}
                    </Button>
                    {net && net.reachability === "cgnat" && (
                      <p className="mt-2 text-warn">
                        {t("fr.cgnat")}
                      </p>
                    )}
                    {net?.public_ip && net.reachability !== "cgnat" && (
                      <p className="mt-2">{t("fr.publicAddr")} <b className="selectable">{net.public_ip}</b>{!net.router_found && <span className="text-muted">{t("fr.noRouter")}</span>}</p>
                    )}
                    {net?.public_ip && net.reachability !== "cgnat" && cur.mode !== m.id && <RealmlistLine host={net.public_ip} />}
                    <Guide
                      steps={[
                        t("fr.g.net.1"),
                        t("fr.g.net.2"),
                        t("fr.g.net.3", { lan: st.lan_ip ?? "?", auth: st.auth_port, world: st.world_port }),
                        t("fr.g.net.4"),
                        t("fr.g.net.5"),
                        t("fr.g.net.6"),
                      ]}
                      links={<ExtLink url="https://portforward.com/router.htm">{t("fr.g.routerHelp")}</ExtLink>}
                    />
                    {net && !net.public_ip && <p className="mt-2 text-warn">{t("fr.noInternet")}</p>}
                  </div>
                )}

                {m.id === "private" && (
                  <p className="mt-2 text-sm">
                    {!st.tailscale.installed && <span className="text-muted">{t("fr.tsMissing")}</span>}
                    {st.tailscale.installed && !st.tailscale.connected && <span className="text-warn">{t("fr.tsOff")}</span>}
                    {st.tailscale.connected && <>{t("fr.privateAddr")} <b className="selectable">{st.tailscale.ip}</b></>}
                  </p>
                )}
                {m.id === "private" && !st.tailscale.installed && (
                  <div className="mt-2 text-sm">
                    <ExtLink url="https://tailscale.com/download">{t("fr.tsDownload")}</ExtLink>
                  </div>
                )}
                {m.id === "private" && st.tailscale.connected && st.tailscale.ip && cur.mode !== m.id && <RealmlistLine host={st.tailscale.ip} />}
                {m.id === "private" && (
                  <Guide
                    steps={[t("fr.g.ts.1"), t("fr.g.ts.2"), t("fr.g.ts.3"), t("fr.g.ts.4")]}
                    links={
                      <>
                        <ExtLink url="https://tailscale.com/download">{t("fr.tsDownload")}</ExtLink>
                        <ExtLink url="https://login.tailscale.com/admin/machines">{t("fr.g.ts.admin")}</ExtLink>
                        <ExtLink url="https://tailscale.com/kb/1084/sharing">{t("fr.g.ts.share")}</ExtLink>
                      </>
                    }
                  />
                )}
              </div>
              <Button
                size="sm"
                variant={m.id === "private" && net?.reachability === "cgnat" ? "primary" : "secondary"}
                disabled={!!busy || (m.id === "lan" && (!lanValid || !selectedLan)) || (m.id === "direct" && !net?.public_ip) || (m.id === "private" && !st.tailscale.connected)}
                onClick={() => void enable(m.id)}
                title={m.id === "direct" && !net?.public_ip ? t("fr.checkFirst") : undefined}
              >
                {busy === `enable-${m.id}` && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
                {cur.mode === m.id ? t("fr.reapply") : t("fr.enable")}
              </Button>
            </div>
          </Card>
        ))}
        {cur.mode !== "local" && (
          <Button variant="ghost" size="sm" className="justify-self-start" disabled={!!busy} onClick={() => void run("local", async () => { await api.friendsEnable(serverId, "local", undefined, false); return t("fr.onlyLocal"); })}>
            {t("fr.stopSharing")}
          </Button>
        )}
      </div>

      {connection && (
        <Card className="mt-6 p-6">
          <h2 className="font-semibold">{t("fr.giveTitle")}</h2>
          <p className="mt-1 text-sm text-muted">{t("fr.giveText")}</p>
          <pre className="selectable mt-3 rounded bg-black/40 p-3 text-sm">{connection}</pre>
          <div className="mt-3 flex flex-wrap gap-2">
            <Button size="sm" onClick={() => void navigator.clipboard.writeText(connection).then(() => { setCopied(true); setTimeout(() => setCopied(false), 2000); })}>
              {copied ? t("fr.copied") : t("fr.copy")}
            </Button>
            <Button size="sm" variant="ghost" disabled={!!busy} onClick={() => void run("pkg", async () => t("fr.pkgSaved", { path: await api.friendsPackage(serverId) }))}>
              {t("fr.pkg")}
            </Button>
          </div>
        </Card>
      )}

      {note && <p className="mt-4 text-sm text-ok" role="status">{note}</p>}
      {needsRestart && st.server_running && (
        <Button
          className="mt-3"
          size="sm"
          variant="primary"
          disabled={!!busy}
          onClick={() =>
            void run("restart", async () => {
              await api.stop(serverId);
              await api.start(serverId);
              setNeedsRestart(false);
            })
          }
        >
          {busy === "restart" && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
          {t("set.restartNow")}
        </Button>
      )}
      {error && <p className="mt-4 text-sm text-bad" role="alert">{error.human.code === "unknown" ? error.technical : human(error.human).message}</p>}
    </div>
  );
}
