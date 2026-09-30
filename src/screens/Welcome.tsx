import { FolderSearch, Download } from "lucide-react";
import { useT } from "@/i18n";
import logo from "@/assets/logo.png";
import { LanguagePicker } from "@/components/LanguagePicker";

export function Welcome({ onImport, onInstall, onBack }: { onImport: () => void; onInstall: () => void; onBack?: () => void }) {
  const t = useT();
  return (
    <main className="relative flex h-full flex-col items-center justify-center px-8">
      <LanguagePicker className="absolute right-6 top-5" />
      <div className="mb-10 text-center">
        <img src={logo} alt="" aria-hidden className="mx-auto mb-5 h-24 w-24 drop-shadow-[0_6px_24px_rgb(201_162_74/0.35)]" />
        <h1 className="text-3xl font-semibold tracking-tight">{t("welcome.title")}</h1>
        <p className="mt-2 text-muted">{t("welcome.subtitle")}</p>
      </div>

      <div className="grid w-full max-w-3xl grid-cols-2 gap-5">
        <button
          onClick={onInstall}
          className="cursor-pointer rounded-card border border-line bg-card/90 p-7 text-left shadow-[0_8px_30px_rgb(0_0_0/0.28)] transition-colors hover:border-gold/60"
        >
          <Download className="mb-4 h-7 w-7 text-gold" aria-hidden />
          <h2 className="text-lg font-semibold">{t("welcome.install.title")}</h2>
          <p className="mt-1 text-sm text-muted">{t("welcome.install.text")}</p>
        </button>

        <button
          onClick={onImport}
          className="cursor-pointer rounded-card border border-line bg-card/90 p-7 text-left shadow-[0_8px_30px_rgb(0_0_0/0.28)] transition-colors hover:border-gold/60"
        >
          <FolderSearch className="mb-4 h-7 w-7 text-gold" aria-hidden />
          <h2 className="text-lg font-semibold">{t("welcome.import.title")}</h2>
          <p className="mt-1 text-sm text-muted">
            {t("welcome.import.text")}
          </p>
        </button>
      </div>
      {onBack && (
        <button onClick={onBack} className="mt-8 cursor-pointer text-sm text-muted hover:text-ink">
          {t("welcome.back")}
        </button>
      )}
    </main>
  );
}
