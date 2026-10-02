import { useCallback, useEffect, useState } from "react";
import { Loader2, RefreshCw } from "lucide-react";
import { api, asUiError, type AccountInfo, type UiError } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { PasswordInput } from "@/components/ui/password-input";
import { useHuman, useT, type Key } from "@/i18n";

const LEVELS = [0, 1, 2, 3] as const;
const inputClass = "w-64 rounded-md border border-line bg-bg px-3 py-2 text-sm outline-none focus:border-gold";

type Editor = { name: string; mode: "password" | "rename" | "delete"; characters?: number };

/** Everyone who can log in to the server, with the changes an owner usually needs: password, access level, name. */
export function AccountsCard({ serverId, online, reloadKey }: { serverId: string; online: boolean | null; reloadKey: number }) {
  const t = useT();
  const human = useHuman();
  const [accounts, setAccounts] = useState<AccountInfo[] | null>(null);
  const [listError, setListError] = useState(false);
  const [editor, setEditor] = useState<Editor | null>(null);
  const [newName, setNewName] = useState("");
  const [confirmName, setConfirmName] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<UiError | null>(null);
  const [note, setNote] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setAccounts(await api.listAccounts(serverId));
      setListError(false);
    } catch {
      setAccounts(null);
      setListError(true);
    }
  }, [serverId]);

  useEffect(() => {
    void load();
  }, [load, reloadKey, online]);

  async function run(label: string, fn: () => Promise<void>, ok: string) {
    setBusy(label);
    setError(null);
    setNote(null);
    try {
      await fn();
      setNote(ok);
      setEditor(null);
      setPassword("");
      setNewName("");
      setConfirmName("");
      await load();
    } catch (e) {
      setError(asUiError(e));
    } finally {
      setBusy(null);
    }
  }

  const passOk = password.length >= 6 && password.length <= 16 && /^[\x21-\x7e]+$/.test(password) && !/["']/.test(password);
  const nameOk = /^[A-Za-z0-9]{3,17}$/.test(newName);

  return (
    <Card className="mt-6 p-6">
      <div className="flex items-start justify-between gap-3">
        <div>
          <h2 className="font-semibold">{t("acc.title")}</h2>
          <p className="mt-1 text-sm text-muted">{t("acc.text")}</p>
        </div>
        <Button size="sm" variant="ghost" onClick={() => void load()} aria-label={t("acc.refresh")} title={t("acc.refresh")}>
          <RefreshCw className="h-4 w-4" aria-hidden />
        </Button>
      </div>

      {listError && <p className="mt-4 text-sm text-warn">{t("acc.unavailable")}</p>}
      {accounts && accounts.length === 0 && <p className="mt-4 text-sm text-muted">{t("acc.empty")}</p>}
      {accounts && accounts.length > 0 && (
        <div className="mt-4 overflow-x-auto">
          <table className="w-full text-left text-sm">
            <thead className="text-xs text-muted">
              <tr>
                <th className="py-2 pr-3 font-normal">{t("acc.col.name")}</th>
                <th className="py-2 pr-3 font-normal">{t("acc.col.chars")}</th>
                <th className="py-2 pr-3 font-normal">{t("acc.col.access")}</th>
                <th className="py-2 pr-3 font-normal">{t("acc.col.last")}</th>
                <th className="py-2 font-normal" />
              </tr>
            </thead>
            <tbody className="divide-y divide-line">
              {accounts.map((a) => (
                <tr key={a.id}>
                  <td className="py-2 pr-3 font-medium">
                    {a.name}
                    {a.online && <span className="ml-2 text-xs text-ok">{t("acc.online")}</span>}
                  </td>
                  <td className="py-2 pr-3">{a.characters}</td>
                  <td className="py-2 pr-3">
                    <select
                      aria-label={t("acc.col.access")}
                      className="rounded-md border border-line bg-bg px-2 py-1 text-sm"
                      value={a.access}
                      disabled={!online || busy !== null}
                      onChange={(e) =>
                        void run(
                          `access:${a.name}`,
                          () => api.accountSetAccess(serverId, a.name, Number(e.target.value)),
                          t("acc.accessChanged", { name: a.name, level: t(`acc.level.${e.target.value}` as Key) }),
                        )
                      }
                    >
                      {LEVELS.map((l) => (
                        <option key={l} value={l}>{t(`acc.level.${l}` as Key)}</option>
                      ))}
                    </select>
                  </td>
                  <td className="py-2 pr-3 text-muted">{a.last_login ?? t("acc.never")}</td>
                  <td className="py-2 text-right whitespace-nowrap">
                    <Button size="sm" variant="ghost" disabled={!online || busy !== null} onClick={() => { setEditor({ name: a.name, mode: "password" }); setError(null); setNote(null); }}>
                      {t("acc.changePassword")}
                    </Button>
                    <Button size="sm" variant="ghost" disabled={!online || busy !== null || a.online} onClick={() => { setEditor({ name: a.name, mode: "rename" }); setError(null); setNote(null); }}>
                      {t("acc.rename")}
                    </Button>
                    <Button size="sm" variant="ghost" className="text-bad hover:text-bad" disabled={!online || busy !== null || a.online} onClick={() => { setEditor({ name: a.name, mode: "delete", characters: a.characters }); setConfirmName(""); setError(null); setNote(null); }}>
                      {t("acc.delete")}
                    </Button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {online === false && accounts && accounts.length > 0 && <p className="mt-3 text-xs text-muted">{t("acc.needsRunning")}</p>}

      {editor && editor.mode === "delete" && (
        <div className="mt-4 rounded-md border border-bad/40 bg-bad/5 p-4" role="alertdialog" aria-label={t("acc.deleteFor", { name: editor.name })}>
          <p className="font-medium text-bad">{t("acc.deleteFor", { name: editor.name })}</p>
          <p className="mt-2 text-sm">{t("acc.deleteWarn", { n: editor.characters ?? 0 })}</p>
          <div className="mt-3">
            <label htmlFor="acc-confirm" className="text-sm text-muted">{t("acc.deleteConfirm", { name: editor.name })}</label>
            <input id="acc-confirm" className={inputClass + " mt-1 block"} value={confirmName} onChange={(e) => setConfirmName(e.target.value)} autoComplete="off" />
          </div>
          <div className="mt-4 flex gap-2">
            <Button
              variant="primary"
              size="sm"
              className="bg-bad text-white hover:bg-bad/90"
              disabled={busy !== null || confirmName.trim().toUpperCase() !== editor.name.toUpperCase()}
              onClick={() => void run("delete", () => api.accountDelete(serverId, editor.name), t("acc.deleted", { name: editor.name }))}
            >
              {busy === "delete" && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
              {t("acc.deleteButton")}
            </Button>
            <Button size="sm" variant="ghost" onClick={() => { setEditor(null); setConfirmName(""); }}>{t("common.cancel")}</Button>
          </div>
        </div>
      )}

      {editor && editor.mode !== "delete" && (
        <div className="mt-4 rounded-md border border-line bg-black/20 p-4">
          <p className="font-medium">{editor.mode === "password" ? t("acc.changeFor", { name: editor.name }) : t("acc.renameFor", { name: editor.name })}</p>
          {editor.mode === "rename" && (
            <div className="mt-3">
              <label htmlFor="acc-newname" className="text-sm text-muted">{t("acc.newName")}</label>
              <input id="acc-newname" className={inputClass + " mt-1 block"} value={newName} onChange={(e) => setNewName(e.target.value)} autoComplete="off" />
              <p className="mt-1 text-xs text-muted">{t("acc.nameRule")}</p>
            </div>
          )}
          <div className="mt-3">
            <label htmlFor="acc-newpass" className="text-sm text-muted">{t("acc.newPassword")}</label>
            <PasswordInput id="acc-newpass" className={inputClass + " mt-1 block"} value={password} onChange={(e) => setPassword(e.target.value)} autoComplete="new-password" />
            <p className="mt-1 text-xs text-muted">{t("acc.passRule")}</p>
          </div>
          {editor.mode === "rename" && <p className="mt-3 text-xs text-muted">{t("acc.renameNote")}</p>}
          <div className="mt-4 flex gap-2">
            <Button
              variant="primary"
              size="sm"
              disabled={busy !== null || !passOk || (editor.mode === "rename" && !nameOk)}
              onClick={() =>
                editor.mode === "password"
                  ? void run("password", () => api.accountSetPassword(serverId, editor.name, password), t("acc.passwordChanged", { name: editor.name }))
                  : void run("rename", () => api.accountRename(serverId, editor.name, newName, password), t("acc.renamed", { name: editor.name, new: newName.toUpperCase() }))
              }
            >
              {busy && <Loader2 className="h-4 w-4 animate-spin" aria-hidden />}
              {t("acc.save")}
            </Button>
            <Button size="sm" variant="ghost" onClick={() => { setEditor(null); setPassword(""); setNewName(""); }}>{t("common.cancel")}</Button>
          </div>
        </div>
      )}

      {note && <p className="mt-3 text-sm text-ok" role="status">{note}</p>}
      {error && <p className="mt-3 text-sm text-bad" role="alert">{error.human.code === "unknown" ? error.technical : human(error.human).message}</p>}
    </Card>
  );
}
