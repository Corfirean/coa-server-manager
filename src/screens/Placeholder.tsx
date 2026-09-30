import { useT } from "@/i18n";

export function Placeholder({ title, question }: { title: string; question: string }) {
  const t = useT();
  return (
    <div className="max-w-xl">
      <h1 className="text-2xl font-semibold">{title}</h1>
      <p className="mt-1 text-muted">{question}</p>
      <p className="mt-8 rounded-card border border-line bg-card/70 p-5 text-sm text-muted">
        {t("placeholder.soon")}
      </p>
    </div>
  );
}
