export type Component = "server" | "manager";
export interface Release { version: string; tag: string; date: string; notes: string[]; prerelease?: boolean; experimentalFeatures?: string[] }
export interface History { component: Component; releases: Release[]; source?: "remote" | "cache" }

// Only stable numeric versions participate in an installed-to-target update range.
export function compareVersions(a: string, b: string): number {
  const aa = a.split(".").map(Number), bb = b.split(".").map(Number);
  for (let i = 0; i < Math.max(aa.length, bb.length); i++) {
    const d = (aa[i] ?? 0) - (bb[i] ?? 0);
    if (d) return d;
  }
  return 0;
}
export function releasesForUpdate(releases: Release[], from?: string, to?: string): Release[] {
  if (!to || !/^\d+(\.\d+)+$/.test(to)) return [];
  const knownFrom = from && /^\d+(\.\d+)+$/.test(from);
  return releases.filter(r => !r.prerelease && /^\d+(\.\d+)+$/.test(r.version) &&
    (knownFrom ? compareVersions(r.version, from) > 0 && compareVersions(r.version, to) <= 0 : r.version === to));
}
export function localizeNotes(notes: string[], dictionary: Record<string, string> = {}) {
  const complete = notes.every(note => dictionary[note]);
  return { notes: complete ? notes.map(note => dictionary[note]) : notes, fallback: !complete };
}
