import { useEffect, useState } from "react";
import { Check, Copy, Eye, EyeOff, Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { useI18n, type Key } from "@/i18n";
import { asControlError, control, type AccountView, type Credentials, type JoinOutcome, type RemoteCharacter } from "@/lib/control";
import { REMOTE_CLIENT_ID } from "@/lib/api";
import { ClientDialog, type ClientDialogMode } from "@/screens/ClientDialog";
import { Notes } from "@/screens/PortablePage";

const STATUS_TEXT: Record<string, Key> = {
  launched: "join.status.launched",
  ready: "join.status.ready",
  needs_relay: "join.status.needsRelay",
  needs_client: "join.status.needsClient",
  needs_transfer: "join.status.needsTransfer",
  incompatible: "join.status.incompatible",
  needs_link: "join.status.needsLink",
};

function ErrorLine({ code, message }: { code: string; message: string }) {
  const { t } = useI18n();
  const key = `join.err.${code}` as Key;
  const known = t(key) !== key;
  return <p className="mt-2 text-sm text-bad" role="alert" data-code={code}>{known ? t(key) : message}</p>;
}

/** The player's account on one server: shown as a name; the password appears only when asked for and goes to the clipboard, never to the screen by default. */
function AccountBox({ realm, account, onForget }: { realm: string; account: AccountView; onForget: () => void }) {
  const { t } = useI18n();
  const [creds, setCreds] = useState<Credentials | null>(null);
  const [shown, setShown] = useState(false);
  const [copied, setCopied] = useState<"name" | "password" | null>(null);
  useEffect(() => { setCreds(null); setShown(false); }, [realm, account.username]);
  async function load(): Promise<Credentials | null> {
    if (creds) return creds;
    const c = await control.credentials(realm);
    setCreds(c);
    return c;
  }
  async function copy(what: "name" | "password") {
    const c = await load();
    if (!c) return;
    try { await navigator.clipboard.writeText(what === "name" ? c.username : c.password); setCopied(what); setTimeout(() => setCopied(null), 4000); } catch { /* the clipboard is not available */ }
  }
  return <div className="rounded-md border border-line p-3" data-testid="account-box">
    <p className="text-xs uppercase tracking-wide text-muted">{t("join.account")}</p>
    <p className="mt-1 flex items-center gap-2 font-mono text-base" data-testid="account-name">{account.username}
      <button aria-label={t("join.copyName")} onClick={() => void copy("name")} className="cursor-pointer text-muted hover:text-ink">{copied === "name" ? <Check className="h-4 w-4 text-ok" /> : <Copy className="h-4 w-4" />}</button>
    </p>
    <div className="mt-2 flex flex-wrap items-center gap-2">
      <Button size="sm" onClick={() => void copy("password")}>{copied === "password" ? <Check className="h-4 w-4 text-ok" aria-hidden /> : <Copy className="h-4 w-4" aria-hidden />}{copied === "password" ? t("join.copied") : t("join.copyPassword")}</Button>
      <Button size="sm" variant="ghost" onClick={() => void load().then(() => setShown((s) => !s))}>{shown ? <EyeOff className="h-4 w-4" aria-hidden /> : <Eye className="h-4 w-4" aria-hidden />}{shown ? t("join.hide") : t("join.show")}</Button>
      <Button size="sm" variant="ghost" onClick={onForget}>{t("join.forget")}</Button>
    </div>
    {shown && creds && <p className="mt-2 break-all font-mono text-sm" data-testid="account-password">{creds.password}</p>}
    <p className="mt-2 text-xs text-muted">{t("join.accountHint")}</p>
  </div>;
}

function LinkForm({ realm, onLinked }: { realm: string; onLinked: () => void }) {
  const { t } = useI18n();
  const [login, setLogin] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<{ code: string; message: string } | null>(null);
  async function submit() {
    setBusy(true); setErr(null);
    try { await control.link(realm, login.trim(), password); setPassword(""); onLinked(); } catch (e) { setErr(asControlError(e)); } finally { setBusy(false); }
  }
  return <form className="rounded-md border border-line p-3" onSubmit={(e) => { e.preventDefault(); void submit(); }}>
    <p className="text-sm font-medium">{t("join.link.title")}</p>
    <p className="mt-1 text-xs text-muted">{t("join.link.text")}</p>
    <label htmlFor="link-login" className="mt-3 block text-sm">{t("join.link.login")}</label>
    <input id="link-login" value={login} autoComplete="off" onChange={(e) => setLogin(e.target.value)} className="mt-1 w-full rounded-md border border-line bg-card px-3 py-2" />
    <label htmlFor="link-password" className="mt-3 block text-sm">{t("join.link.password")}</label>
    <input id="link-password" type="password" value={password} autoComplete="off" onChange={(e) => setPassword(e.target.value)} className="mt-1 w-full rounded-md border border-line bg-card px-3 py-2" />
    <Button type="submit" className="mt-3" disabled={busy || login.trim() === "" || password === ""}>{busy && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}{t("join.link.submit")}</Button>
    {err && <ErrorLine code={err.code} message={err.message} />}
  </form>;
}

const CLASS_NAMES: Record<number, string> = { 1: "Warrior", 2: "Paladin", 3: "Hunter", 4: "Rogue", 5: "Priest", 6: "Death Knight", 7: "Shaman", 8: "Mage", 9: "Warlock", 11: "Druid" };

function ServerCharacters({ realm, onClaimed }: { realm: string; onClaimed: () => void }) {
  const { t } = useI18n();
  const [list, setList] = useState<RemoteCharacter[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [err, setErr] = useState<{ code: string; message: string } | null>(null);
  const [done, setDone] = useState<string | null>(null);
  async function load() {
    setBusy("*"); setErr(null);
    try { setList(await control.characters(realm)); } catch (e) { setErr(asControlError(e)); setList(null); } finally { setBusy(null); }
  }
  async function claim(name: string) {
    setBusy(name); setErr(null); setDone(null);
    try { const c = await control.claim(realm, name); setDone(t("join.chars.done", { name: c.name, revision: c.revision })); onClaimed(); await load(); } catch (e) { setErr(asControlError(e)); } finally { setBusy(null); }
  }
  return <section data-testid="server-characters">
    <h3 className="mb-1 font-medium">{t("join.chars.title")}</h3>
    <p className="text-xs text-muted">{t("join.chars.text")}</p>
    <Button size="sm" className="mt-2" onClick={() => void load()} disabled={busy !== null}>{busy === "*" && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}{t("join.chars.show")}</Button>
    {list && list.length === 0 && <p className="mt-2 text-sm text-muted">{t("join.chars.none")}</p>}
    {list && list.length > 0 && <ul className="mt-2 space-y-2">
      {list.map((c) => <li key={c.name} className="flex items-center justify-between gap-3 rounded-md border border-line p-2">
        <span><span className="font-medium">{c.name}</span><span className="text-muted"> — {CLASS_NAMES[c.class] ?? `#${c.class}`}, {t("portable.level", { level: c.level })}</span>
          {!c.eligible && <span className="block text-xs text-warn">{t("join.chars.notEligible")} ({c.reasons.join(", ")})</span>}</span>
        {c.yours ? <span className="text-xs text-ok">{t("join.chars.portable")}</span>
          : <Button size="sm" disabled={!c.eligible || busy !== null} onClick={() => void claim(c.name)}>{busy === c.name && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}{t("join.chars.make")}</Button>}
      </li>)}
    </ul>}
    {done && <p className="mt-2 text-sm text-ok" role="status">{done}</p>}
    {err && <ErrorLine code={err.code} message={err.message} />}
  </section>;
}

/** Joining one server: the account, the game, the character-claim and the honest "not yet" cases. */
export function JoinPanel({ realm, character, automatic, onClaimed }: { realm: string; character: string; automatic: boolean; onClaimed: () => void }) {
  const { t } = useI18n();
  const [account, setAccount] = useState<AccountView | null>(null);
  const [busy, setBusy] = useState(false);
  const [outcome, setOutcome] = useState<JoinOutcome | null>(null);
  const [err, setErr] = useState<{ code: string; message: string } | null>(null);
  const [linking, setLinking] = useState(false);
  const [bring, setBring] = useState(false);
  const [clientDialog, setClientDialog] = useState<ClientDialogMode | null>(null);
  const refresh = () => control.account(realm).then(setAccount).catch(() => setAccount(null));
  useEffect(() => { setOutcome(null); setErr(null); setLinking(false); void refresh(); }, [realm]); // eslint-disable-line react-hooks/exhaustive-deps

  async function join() {
    setBusy(true); setErr(null); setOutcome(null);
    try {
      const o = await control.join(realm, bring && character ? character : null, true);
      setOutcome(o);
      if (o.status === "needs_link") setLinking(true);
      if (o.status === "needs_client") setClientDialog("setup");
      await refresh();
    } catch (e) { setErr(asControlError(e)); } finally { setBusy(false); }
  }

  const text = outcome ? STATUS_TEXT[outcome.status] : undefined;
  const bad = outcome && ["incompatible", "needs_relay", "needs_client", "needs_transfer", "needs_link"].includes(outcome.status);
  return <div className="space-y-4">
    {account && <AccountBox realm={realm} account={account} onForget={() => void control.forget(realm).then(() => { setAccount(null); setOutcome(null); })} />}
    {(linking || (!account && !automatic)) && <LinkForm realm={realm} onLinked={() => { setLinking(false); void refresh(); setOutcome(null); }} />}
    <div>
      {character && <label className="mb-2 flex items-start gap-2 text-sm"><input type="checkbox" checked={bring} onChange={(e) => setBring(e.target.checked)} className="mt-1" data-testid="bring-character" /><span>{t("join.bring")}<span className="block text-xs text-muted">{t("join.bringHint")}</span></span></label>}
      <Button variant="primary" className="w-full" disabled={busy} onClick={() => void join()} data-testid="join-button">{busy && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}{t("join.button")}</Button>
      {!account && automatic && !linking && <button className="mt-2 cursor-pointer text-xs text-muted underline hover:text-ink" onClick={() => setLinking(true)}>{t("join.haveAccount")}</button>}
      {outcome && text && <div className={cn("mt-3 rounded-md border p-3 text-sm", bad ? "border-warn/50 bg-warn/10" : "border-ok/40 bg-ok/10")} role="status" data-status={outcome.status}>
        <p>{t(text)}</p>
        {outcome.status === "needs_client" && (
          <Button size="sm" className="mt-2" onClick={() => setClientDialog("setup")}>
            {t("btn.setupClient")}
          </Button>
        )}
        {outcome.account_created && <p className="mt-1 text-xs text-muted">{t("join.created", { name: outcome.username ?? "" })}</p>}
        {outcome.password_reset && <p className="mt-1 text-xs text-muted">{t("join.reset")}</p>}
        {outcome.notes.length > 0 && <Notes notes={outcome.notes} />}
      </div>}
      {err && <ErrorLine code={err.code} message={err.message} />}
    </div>
    <ServerCharacters realm={realm} onClaimed={onClaimed} />
    {clientDialog && (
      <ClientDialog
        serverId={REMOTE_CLIENT_ID}
        mode={clientDialog}
        onClose={() => setClientDialog(null)}
        onChanged={() => {
          setClientDialog(null);
          void join();
        }}
      />
    )}
  </div>;
}
