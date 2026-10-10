export interface UnsavedChangesGuard {
  isDirty: () => boolean;
  save: () => Promise<boolean>;
  discard: () => void;
}
