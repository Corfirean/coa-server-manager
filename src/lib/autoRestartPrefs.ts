import { useState } from "react";

const key = (id: string) => `coa-auto-restart-on-crash:${id}`;
const fallback = () => false;

export function autoRestartOnCrash(id: string): boolean {
  try {
    const value = localStorage.getItem(key(id));
    return value === null ? fallback() : value === "1";
  } catch {
    return fallback();
  }
}

export function setAutoRestartOnCrash(id: string, on: boolean) {
  try {
    localStorage.setItem(key(id), on ? "1" : "0");
  } catch {
    /* storage unavailable */
  }
}

export function useAutoRestart(id: string): [boolean, (on: boolean) => void] {
  const [on, setOn] = useState(() => autoRestartOnCrash(id));
  return [
    on,
    (next) => {
      setAutoRestartOnCrash(id, next);
      setOn(next);
    },
  ];
}
