/** The text of a problem report, always in English so whoever reads it on GitHub can follow it; the person's own words go in as typed. */
export interface ReportInput {
  managerVersion: string;
  windows: string;
  installKind: "new" | "imported";
  serverVersion: string | null;
  what: string;
  expected: string;
  steps: string;
  /** a diagnostics file was created for this report */
  diagnostics: boolean;
}

export const NEW_ISSUE_URL = "https://github.com/Corfirean/coa-server-manager/issues/new";
const PRIVACY = "> Please remove passwords, tokens, private IP information or other sensitive data before posting logs.";

export function reportBody(r: ReportInput): string {
  const text = (s: string, fallback: string) => (s.trim() ? s.trim() : fallback);
  return [
    "## Bug Report",
    "",
    `**Manager version:** ${r.managerVersion || "unknown"}  `,
    `**Windows version:** ${r.windows || "unknown"}  `,
    `**Fresh install or imported server:** ${r.installKind === "new" ? "Fresh install" : "Imported server"}  `,
    ...(r.serverVersion ? [`**Server version:** ${r.serverVersion}  `] : []),
    "",
    "**What happened?**  ",
    text(r.what, "Describe the problem."),
    "",
    "**What did you expect to happen?**  ",
    text(r.expected, "Describe the expected behaviour."),
    "",
    "**Steps to reproduce:**  ",
    text(r.steps, "1.\n2.\n3."),
    "",
    "**Screenshot / video:**  ",
    "Attach it if possible.",
    "",
    "**Logs:**  ",
    r.diagnostics ? "The diagnostics file from the Manager is attached below (it has passwords and addresses removed)." : "Attach relevant Manager/server logs if available.",
    "",
    PRIVACY,
    "",
  ].join("\n");
}

/** Percent-encode everything that is not a plain letter, digit or one of . _ - so the link carries nothing the browser could misread. */
export function encodeQuery(s: string): string {
  return Array.from(new TextEncoder().encode(s))
    .map((b) => {
      const c = String.fromCharCode(b);
      return /[A-Za-z0-9._-]/.test(c) ? c : "%" + b.toString(16).toUpperCase().padStart(2, "0");
    })
    .join("");
}

/** A link that opens GitHub's "new issue" page with the title and text filled in. */
export function issueUrl(title: string, body: string): string {
  return `${NEW_ISSUE_URL}?labels=bug&title=${encodeQuery(title)}&body=${encodeQuery(body)}`;
}

/** Links longer than this are not reliable in every browser; the full text is then copied to the clipboard instead. */
export const MAX_LINK = 6500;
