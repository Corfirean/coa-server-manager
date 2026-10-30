export class ServerLifecycle {
  private wasRunning = false;
  private maintenance = false;
  private stopRequested = false;
  private operationPending = false;

  request(action: "starting" | "stopping" | "restarting") {
    this.stopRequested = action !== "starting";
    this.operationPending = true;
  }

  complete(action: "starting" | "stopping" | "restarting") {
    this.operationPending = false;
    if (action !== "stopping") { this.stopRequested = false; this.wasRunning = false; }
  }

  beginMaintenance() { this.maintenance = true; this.wasRunning = false; }
  endMaintenance() { this.maintenance = false; this.wasRunning = false; }

  observe(running: boolean, busy: boolean, updating: boolean): boolean {
    if (this.maintenance) { this.wasRunning = false; return false; }
    if (running) { this.wasRunning = true; return false; }
    if (this.stopRequested || updating || this.operationPending) {
      this.wasRunning = false;
      if (!busy && !this.operationPending) this.stopRequested = false;
      return false;
    }
    if (this.wasRunning && !busy) { this.wasRunning = false; return true; }
    return false;
  }
}

const lifecycles = new Map<string, ServerLifecycle>();
export function serverLifecycle(id: string) {
  let lifecycle = lifecycles.get(id);
  if (!lifecycle) { lifecycle = new ServerLifecycle(); lifecycles.set(id, lifecycle); }
  return lifecycle;
}
