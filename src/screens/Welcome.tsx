import { FolderSearch, Download } from "lucide-react";

export function Welcome({ onImport, onInstall, onBack }: { onImport: () => void; onInstall: () => void; onBack?: () => void }) {
  return (
    <main className="flex h-full flex-col items-center justify-center px-8">
      <div className="mb-10 text-center">
        <div className="mx-auto mb-5 h-12 w-12 rounded-full border-2 border-gold/80 bg-card-2" aria-hidden />
        <h1 className="text-3xl font-semibold tracking-tight">Welcome to CoA Server Manager</h1>
        <p className="mt-2 text-muted">Run your own Conquest of Azeroth server — no technical knowledge needed.</p>
      </div>

      <div className="grid w-full max-w-3xl grid-cols-2 gap-5">
        <button
          onClick={onInstall}
          className="cursor-pointer rounded-card border border-line bg-card/90 p-7 text-left shadow-[0_8px_30px_rgb(0_0_0/0.28)] transition-colors hover:border-gold/60"
        >
          <Download className="mb-4 h-7 w-7 text-gold" aria-hidden />
          <h2 className="text-lg font-semibold">Install new server</h2>
          <p className="mt-1 text-sm text-muted">One-click setup with everything included.</p>
        </button>

        <button
          onClick={onImport}
          className="cursor-pointer rounded-card border border-line bg-card/90 p-7 text-left shadow-[0_8px_30px_rgb(0_0_0/0.28)] transition-colors hover:border-gold/60"
        >
          <FolderSearch className="mb-4 h-7 w-7 text-gold" aria-hidden />
          <h2 className="text-lg font-semibold">I already have a server</h2>
          <p className="mt-1 text-sm text-muted">
            Add your existing server. It is only read — nothing is changed.
          </p>
        </button>
      </div>
      {onBack && (
        <button onClick={onBack} className="mt-8 cursor-pointer text-sm text-muted hover:text-ink">
          Back to my servers
        </button>
      )}
    </main>
  );
}
