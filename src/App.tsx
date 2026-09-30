import { useCallback, useEffect, useState } from "react";
import { api, type ServerSummary } from "@/lib/api";
import { Welcome } from "@/screens/Welcome";
import { ImportServer } from "@/screens/ImportServer";
import { Shell } from "@/screens/Shell";

type View = "welcome" | "import";

export default function App() {
  const [servers, setServers] = useState<ServerSummary[] | null>(null);
  const [view, setView] = useState<View>("welcome");
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

  if (servers.length > 0 && view === "welcome" && activeId) {
    return (
      <Shell
        servers={servers}
        activeId={activeId}
        onSelect={setActiveId}
        onAddAnother={() => setView("import")}
        onForget={async (id) => {
          await api.forget(id);
          await refresh();
        }}
      />
    );
  }

  if (view === "import") {
    return (
      <ImportServer
        canCancel={servers.length > 0}
        onCancel={() => setView("welcome")}
        onAdded={async (s) => {
          await refresh();
          setActiveId(s.id);
          setView("welcome");
        }}
      />
    );
  }

  return <Welcome onImport={() => setView("import")} />;
}
