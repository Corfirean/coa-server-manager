export function Placeholder({ title, question }: { title: string; question: string }) {
  return (
    <div className="max-w-xl">
      <h1 className="text-2xl font-semibold">{title}</h1>
      <p className="mt-1 text-muted">{question}</p>
      <p className="mt-8 rounded-card border border-line bg-card/70 p-5 text-sm text-muted">
        This section is being built in a later release.
      </p>
    </div>
  );
}
