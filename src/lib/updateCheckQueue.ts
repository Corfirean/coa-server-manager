export function updateCheckQueue<T>() {
  const requests = new Map<string, Promise<T>>();
  const servers = new Map<string, Promise<unknown>>();
  return (server: string, request: string, run: () => Promise<T>): Promise<T> => {
    const key = JSON.stringify([server, request]);
    const existing = requests.get(key);
    if (existing) return existing;
    const previous = servers.get(server) ?? Promise.resolve();
    const result = previous.catch(() => undefined).then(run);
    requests.set(key, result);
    servers.set(server, result);
    const clear = () => {
      if (requests.get(key) === result) requests.delete(key);
      if (servers.get(server) === result) servers.delete(server);
    };
    void result.then(clear, clear);
    return result;
  };
}
