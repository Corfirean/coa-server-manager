import { useEffect, useId, useRef, useState, type KeyboardEvent } from "react";
import { ChevronDown } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useI18n } from "@/i18n";
import type { PbcModel } from "@/lib/api";
import { cn } from "@/lib/utils";

export type ModelCatalog = {
  status: "idle" | "loading" | "ready" | "error";
  models: PbcModel[];
};

const fieldClass = "selectable w-full rounded-md border border-line bg-bg px-3 py-2 text-sm outline-none focus:border-gold";

export function PbcModelPicker({ value, onChange, catalog, discoveryBlocked, loadModels }: {
  value: string;
  onChange: (id: string) => void;
  catalog: ModelCatalog;
  discoveryBlocked?: string;
  loadModels: (refresh: boolean) => void;
}) {
  const { t } = useI18n();
  const id = useId();
  const [open, setOpen] = useState(false);
  const [manual, setManual] = useState(false);
  const [query, setQuery] = useState("");
  const [highlight, setHighlight] = useState(-1);
  const trigger = useRef<HTMLButtonElement>(null);
  const search = useRef<HTMLInputElement>(null);
  const manualInput = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLUListElement>(null);
  const term = query.trim().toLocaleLowerCase();
  const models = catalog.models.filter(model => `${model.name} ${model.id}`.toLocaleLowerCase().includes(term));
  const selected = catalog.models.find(model => model.id === value);
  const active = Math.min(highlight, models.length);

  useEffect(() => {
    if (open) search.current?.focus();
    else if (manual) manualInput.current?.focus();
  }, [open, manual]);

  useEffect(() => {
    if (open && active >= 0) list.current?.children.item(active)?.scrollIntoView({ block: "nearest" });
  }, [open, active]);

  function browse(refresh = false) {
    setManual(false);
    if (!open) {
      setQuery("");
      setHighlight(-1);
      setOpen(true);
    } else {
      search.current?.focus();
    }
    loadModels(refresh);
  }

  function choose(index: number) {
    setOpen(false);
    if (index === models.length) {
      setManual(true);
    } else {
      onChange(models[index].id);
      trigger.current?.focus();
    }
  }

  function navigate(event: KeyboardEvent<HTMLInputElement>) {
    switch (event.key) {
      case "ArrowDown":
      case "ArrowUp":
        event.preventDefault();
        setHighlight(event.key === "ArrowDown"
          ? Math.min(active + 1, models.length)
          : active <= 0 ? models.length : active - 1);
        break;
      case "Enter":
        event.preventDefault();
        choose(Math.max(0, active));
        break;
      case "Escape":
        event.preventDefault();
        event.stopPropagation();
        setOpen(false);
        trigger.current?.focus();
        break;
    }
  }

  const status = discoveryBlocked
    ?? (catalog.status === "loading" ? t("mod.pbc.loadingModels")
      : catalog.status === "error" ? t("mod.pbc.modelListFailed")
      : !models.length && catalog.status === "ready" ? t("mod.pbc.noModels")
      : "");

  return (
    <div className="space-y-1.5 sm:col-span-2" onBlur={event => {
      if (!event.currentTarget.contains(event.relatedTarget)) setOpen(false);
    }}>
      <label htmlFor={id} className="block text-sm">{t("mod.pbc.model")}</label>
      <div className="flex flex-wrap items-start gap-2">
        <div className="relative min-w-56 flex-1">
          {manual ? (
            <input ref={manualInput} id={id} className={fieldClass} value={value} required
              placeholder={t("mod.pbc.manualModel")} onChange={event => onChange(event.target.value)} />
          ) : (
            <button ref={trigger} id={id} type="button" className={cn(fieldClass, "flex items-center justify-between gap-2 text-left")}
              aria-expanded={open} aria-controls={open ? `${id}-picker` : undefined}
              onClick={() => open ? setOpen(false) : browse()}
              onKeyDown={event => {
                if (!open && (event.key === "ArrowDown" || event.key === "ArrowUp")) {
                  event.preventDefault();
                  browse();
                }
              }}>
              <span className={cn("truncate", !value && "text-muted")}>
                {selected?.name ?? (value || t("mod.pbc.chooseModel"))}
              </span>
              <ChevronDown className="h-4 w-4 shrink-0 text-muted" aria-hidden />
            </button>
          )}
          {open && (
            <div id={`${id}-picker`} className="absolute z-20 mt-1 w-full rounded-md border border-line bg-bg p-2 shadow-lg">
              <input ref={search} type="text" role="combobox" aria-expanded="true" aria-autocomplete="list"
                aria-controls={`${id}-options`} aria-activedescendant={active >= 0 ? `${id}-option-${active}` : undefined}
                aria-label={t("mod.pbc.searchModels")} placeholder={t("mod.pbc.searchModels")}
                className={fieldClass} value={query} autoComplete="off"
                onChange={event => { setQuery(event.target.value); setHighlight(-1); }} onKeyDown={navigate} />
              {status && <p className="px-2 py-2 text-sm text-muted" role="status">{status}</p>}
              <ul ref={list} id={`${id}-options`} role="listbox" aria-label={t("mod.pbc.model")} className="mt-1 max-h-56 overflow-auto">
                {models.map((model, index) => (
                  <li key={model.id} id={`${id}-option-${index}`} role="option" aria-selected={model.id === value}
                    className={cn("cursor-pointer rounded px-2 py-1.5 text-sm hover:bg-gold/10", active === index && "bg-gold/15")}
                    onMouseDown={event => event.preventDefault()} onClick={() => choose(index)}>
                    <span className="block break-all">{model.name}</span>
                    {model.id !== model.name && <span className="block break-all text-xs text-muted">{model.id}</span>}
                  </li>
                ))}
                <li id={`${id}-option-${models.length}`} role="option" aria-selected={false}
                  className={cn("cursor-pointer rounded border-t border-line px-2 py-1.5 text-sm text-gold hover:bg-gold/10",
                    active === models.length && "bg-gold/15")}
                  onMouseDown={event => event.preventDefault()} onClick={() => choose(models.length)}>
                  {t("mod.pbc.manualModel")}
                </li>
              </ul>
            </div>
          )}
        </div>
        <Button size="sm" variant="secondary" type="button" disabled={!manual && (!!discoveryBlocked || catalog.status === "loading")}
          onClick={() => manual ? browse() : browse(true)}>
          {t(manual ? "mod.pbc.browseModels" : "mod.pbc.refreshModels")}
        </Button>
      </div>
    </div>
  );
}
