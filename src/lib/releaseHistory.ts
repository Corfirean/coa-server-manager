import manager from "../../changelog/history.json";
import server from "@/data/server-history.json";
import translations from "@/data/latest-note-translations.json";
import type { Locale } from "@/i18n";

import { localizeNotes, type Component, type History, type Release } from "./releaseHistoryModel";
export { compareVersions, releasesForUpdate } from "./releaseHistoryModel";
export type { Component, History } from "./releaseHistoryModel";
export const bundledHistories: Record<Component, History> = { manager: manager as History, server: server as History };
export const repositories = { manager: "Corfirean/coa-server-manager", server: "Corfirean/coa-server-build" };

export function translatedNotes(component: Component, release: Release, locale: Locale): { notes: string[]; fallback: boolean } {
  if (locale === "en") return { notes: release.notes, fallback: false };
  const dict = (translations[component] as Record<string, Record<string, string>>)[locale] ?? {};
  return localizeNotes(release.notes, dict);
}
