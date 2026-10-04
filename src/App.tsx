import { useCallback, useEffect, useState } from "react";
import { api, type ServerSummary } from "@/lib/api";
import { Welcome } from "@/screens/Welcome";
import { ImportServer } from "@/screens/ImportServer";
import { Shell } from "@/screens/Shell";
import { InstallServer } from "@/screens/InstallServer";
import { RemoteClient } from "@/screens/RemoteClient";
import { UpdateBanner } from "@/components/UpdateBanner";
import { autoUpdateEnabled, installOnClose, lookForUpdate } from "@/lib/selfUpdate";

type View = "welcome" | "add" | "import" | "install" | "remote";

function initialView(): View {
  try { return localStorage.getItem("coa-play-mode") === "remote" ? "remote" : "welcome"; }
  catch { return "welcome"; }
}

function Main() {
  const [servers, setServers] = useState<ServerSummary[] | null>(null);
  const [view, setView] = useState<View>(initialView);
  const [activeId, setActiveId] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    const list = await api.list().catch(() => []);
    setServers(list);
    setActiveId((cur) => (cur && list.some((s) => s.id === cur) ? cur : (list[0]?.id ?? null)));
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  if (servers === null) return null;

  if (view === "remote") return <RemoteClient onHost={() => {
    try { localStorage.setItem("coa-play-mode", "host"); } catch { /* storage unavailable */ }
    setView(servers.length ? "welcome" : "add");
  }} />;

  if (servers.length > 0 && view === "welcome" && activeId) {
    return (
      <Shell
        servers={servers}
        activeId={activeId}
        onSelect={setActiveId}
        onAddAnother={() => setView("add")}
        onForget={async (id) => {
          await api.forget(id);
          await refresh();
        }}
      />
    );
  }

  if (view === "install") {
    return (
      <InstallServer
        canCancel
        onCancel={() => setView(servers.length > 0 ? "add" : "welcome")}
        onDone={async (s) => {
          await refresh();
          setActiveId(s.id);
          setView("welcome");
        }}
      />
    );
  }

  if (view === "import") {
    return (
      <ImportServer
        canCancel
        onCancel={() => setView(servers.length > 0 ? "add" : "welcome")}
        onAdded={async (s) => {
          await refresh();
          setActiveId(s.id);
          setView("welcome");
        }}
      />
    );
  }

  return (
    <Welcome
      onConnect={() => {
        try { localStorage.setItem("coa-play-mode", "remote"); } catch { /* storage unavailable */ }
        setView("remote");
      }}
      onImport={() => setView("import")}
      onInstall={() => setView("install")}
      onBack={servers.length > 0 ? () => setView("welcome") : undefined}
    />
  );
}

export default function App() {
  // Quietly fetch a new Manager version a few seconds after start and install it when the window is closed.
  useEffect(() => {
    const stop = installOnClose();
    const timer = setTimeout(() => {
      if (autoUpdateEnabled()) void lookForUpdate({ download: true });
    }, 5000);
    return () => {
      clearTimeout(timer);
      stop();
    };
  }, []);

  return (
    <>
      <Main />
      <UpdateBanner />
    </>
  );
}
