import type { Ruleset } from "@/lib/browse";
import { cn } from "@/lib/utils";

interface RulesetBadgeProps {
  ruleset: Ruleset | string;
  className?: string;
  size?: "sm" | "md";
}

export function formatRulesetLabel(ruleset: Ruleset | string): string {
  switch (ruleset) {
    case "coa":
      return "Conquest of Azeroth";
    case "wildcard":
      return "Wildcard";
    case "reborn":
      return "Warcraft Reborn";
    default:
      return String(ruleset);
  }
}

export function RulesetBadge({ ruleset, className, size = "md" }: RulesetBadgeProps) {
  const label = formatRulesetLabel(ruleset);

  let style = "bg-white/10 text-ink/90 border-line";
  if (ruleset === "coa") {
    // Conquest of Azeroth accent: subtle blue/cyan
    style = "bg-sky-500/10 text-sky-300 border-sky-500/30";
  } else if (ruleset === "wildcard") {
    // Wildcard accent: subtle amber/gold
    style = "bg-amber-500/10 text-amber-300 border-amber-500/30";
  } else if (ruleset === "reborn") {
    // Warcraft Reborn accent: subtle emerald/green
    style = "bg-emerald-500/10 text-emerald-300 border-emerald-500/30";
  } else {
    // Unknown ruleset: red warning border/tone to fail visibly and safely
    style = "bg-bad/15 text-bad border-bad/40";
  }

  const sizeClass = size === "sm" ? "px-2 py-0.5 text-[11px]" : "px-2.5 py-0.5 text-xs";

  return (
    <span
      data-testid="ruleset-badge"
      data-ruleset={ruleset}
      className={cn(
        "inline-flex items-center font-medium rounded-full border select-none tracking-wide",
        sizeClass,
        style,
        className
      )}
    >
      {label}
    </span>
  );
}
