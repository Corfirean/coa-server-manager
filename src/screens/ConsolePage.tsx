import { useCallback, useEffect, useRef, useState } from "react";
import { api, asUiError, type ConsoleLine, type ConsoleSource, type UiError } from "@/lib/api";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";

const TABS: { id: ConsoleSource; label: string }[] = [
  { id: "world", label: "World" },
  { id: "auth", label: "Auth" },
  { id: "database", label: "Database" },
  { id: "manager", label: "Manager" },
];

const TONE = { info: "text-ink/80", warn: "text-warn", error: "text-bad" } as const;

export function ConsolePage({ serverId }: { serverId: string }) {
  const [tab, setTab] = useState<ConsoleSource>("world");
  const [filter, setFilter] = useState("");
  const [lines, setLines] = useState<ConsoleLine[]>([]);
  const [live, setLive] = useState(true);
  const [missing, setMissing] = useState<string | null>(null);
  const [cmd, setCmd] = useState("");
  const [reply, setReply] = useState<string | null>(null);
  const [error, setError] = useState<UiError | null>(null);
  const [copied, setCopied] = useState(false);
  const box = useRef<HTMLDivElement>(null);

  const load = useCallback(async () => {
    try {
      setLines(await api.consoleTail(serverId, tab, filter || undefined));
      setMissing(null);
    } catch (e) {
      setLines([]);
      setMissing(asUiError(e).technical);
    }
  }, [serverId, tab, filter]);

  useEffect(() => {
    void load();
    if (!live) return;
    const t = setInterval(load, 2000);
    return () => clearInterval(t);
  }, [load, live]);

  useEffect(() => {
    if (live && box.current) box.current.scrollTop = box.current.scrollHeight;
  }, [lines, live]);

  async function send() {
    setError(null);
    setReply(null);
    try {
      const risk = await api.consoleRisk(cmd);
      if (risk === "dangerous" && !window.confirm(`“${cmd}” can shut things down or change many records. Send it anyway?`)) return;
      setReply((await api.consoleCommand(serverId, cmd, risk === "dangerous")) || "(no output)");
      setCmd("");
    } catch (e) {
      setError(asUiError(e));
    }
  }

  const errors = lines.filter((l) => l.level === "error").map((l) => l.text);

  return (
    <div className="flex h-full max-w-4xl flex-col">
      <h1 className="text-2xl font-semibold">Console</h1>
      <p className="mt-1 text-muted">What is the server doing?</p>

      <div className="mt-4 flex flex-wrap items-center gap-2">
        <div className="flex gap-1" role="tablist">
          {TABS.map((t) => (
            <button
              key={t.id}
              role="tab"
              aria-selected={tab === t.id}
              onClick={() => setTab(t.id)}
              className={cn("cursor-pointer rounded-md px-3 py-1.5 text-sm", tab === t.id ? "bg-white/10 text-ink" : "text-muted hover:text-ink")}
            >
              {t.label}
            </button>
          ))}
        </div>
        <input
          aria-label="Search the log"
          placeholder="Search…"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          className="ml-auto w-56 rounded-md border border-line bg-bg px-3 py-1.5 text-sm outline-none focus:border-gold"
        />
        <label className="flex cursor-pointer items-center gap-1.5 text-sm text-muted">
          <input type="checkbox" checked={live} onChange={(e) => setLive(e.target.checked)} /> Follow
        </label>
        <Button
          size="sm"
          variant="ghost"
          disabled={errors.length === 0}
          onClick={() => void navigator.clipboard.writeText(errors.slice(-20).join("\n")).then(() => { setCopied(true); setTimeout(() => setCopied(false), 1500); })}
        >
          {copied ? "Copied" : "Copy errors"}
        </Button>
      </div>

      <div ref={box} className="selectable mt-3 min-h-64 flex-1 overflow-auto rounded-card border border-line bg-black/40 p-3 font-mono text-xs leading-5" role="log" aria-live="off">
        {missing ? <p className="text-muted">{missing}</p> : lines.length === 0 ? <p className="text-muted">Nothing to show.</p> : lines.map((l, i) => (
          <div key={i} className={cn("whitespace-pre-wrap break-all", TONE[l.level])}>
            {l.level !== "info" && <span className="sr-only">{l.level}: </span>}
            {l.text}
          </div>
        ))}
      </div>

      {tab === "world" && (
        <Card className="mt-3 p-4">
          <p className="text-xs text-muted">Commands go straight to the running world server. Risky ones (shutdown, deleting or banning) ask for confirmation.</p>
          <form className="mt-2 flex gap-2" onSubmit={(e) => { e.preventDefault(); void send(); }}>
            <input aria-label="Command" value={cmd} onChange={(e) => setCmd(e.target.value)} placeholder="server info" className="selectable flex-1 rounded-md border border-line bg-bg px-3 py-2 font-mono text-sm outline-none focus:border-gold" />
            <Button variant="secondary" size="sm" type="submit" disabled={!cmd.trim()}>Send</Button>
          </form>
          {reply && <pre className="selectable mt-2 max-h-40 overflow-auto whitespace-pre-wrap rounded bg-black/40 p-2 text-xs">{reply}</pre>}
          {error && <p className="mt-2 text-sm text-bad" role="alert">{error.human.code === "unknown" ? error.technical : error.human.message}</p>}
        </Card>
      )}
    </div>
  );
}
