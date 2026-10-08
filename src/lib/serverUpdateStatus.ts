import type { UpdatePreview } from "./api";

export function isUpdateCurrent(preview: UpdatePreview): boolean {
  return preview.from_version === preview.to_version
    && preview.items.every((item) => item.action === "skip")
    && (preview.database_check_error !== undefined || preview.database_checked === false || (preview.pending_migrations ?? (preview.migrations === 0 ? 0 : undefined)) === 0);
}
