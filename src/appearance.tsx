import { t } from "./language";
import { useSyncExternalStore } from "react";
import { listen } from "@tauri-apps/api/event";
import { Monitor, Moon, Sun } from "lucide-react";
import { call, native } from "./api";

type Appearance = "system" | "light" | "dark";
// Keep the existing storage key so the Ilum rename preserves appearance preferences.
const preferenceKey = "the-bad-batch.appearance";
const systemDark = window.matchMedia("(prefers-color-scheme: dark)");
const subscribers = new Set<() => void>();

function normalize(value: unknown): Appearance {
  return value === "light" || value === "dark" ? value : "system";
}

function readPreference() {
  try { return normalize(window.localStorage.getItem(preferenceKey)); }
  catch { return "system"; }
}

let selection = readPreference();

function apply() {
  const dark = selection === "dark" || (selection === "system" && systemDark.matches);
  document.documentElement.dataset.appearance = dark ? "dark" : "light";
}

function update(value: Appearance) {
  selection = value;
  apply();
  subscribers.forEach(notify => notify());
}

export function initializeAppearance(mainWindow: boolean) {
  apply();
  systemDark.addEventListener("change", apply);
  window.addEventListener("storage", event => {
    if (event.key === preferenceKey || event.key === null) update(readPreference());
  });
  if (native) {
    void listen<Appearance>("appearance-changed", event => update(normalize(event.payload))).catch(() => {});
    if (mainWindow && selection !== "system") void call("set_appearance", { mode: selection }).catch(() => {});
  }
}

function choose(value: Appearance) {
  if (selection === value) return;
  try { window.localStorage.setItem(preferenceKey, value); }
  catch { /* Current session still uses the selected appearance. */ }
  update(value);
  if (native) void call("set_appearance", { mode: value }).catch(() => {});
}

function subscribe(notify: () => void) {
  subscribers.add(notify);
  return () => subscribers.delete(notify);
}

const options = [
  { key: "system", label: "System", title: "macOS-Erscheinungsbild übernehmen", icon: Monitor },
  { key: "light", label: "Hell", title: "Helles Erscheinungsbild", icon: Sun },
  { key: "dark", label: "Dunkel", title: "Dunkles Erscheinungsbild", icon: Moon },
] as const;

export function AppearanceControl() {
  const selected = useSyncExternalStore(subscribe, () => selection);
  return <div className="appearance-control" role="group" aria-label={t("Erscheinungsbild")} data-tauri-drag-region="false">
    {options.map(({ key, label, title, icon: Icon }) => <button type="button" key={key}
      title={t(title)} aria-label={t(label)} aria-pressed={selected === key} onClick={() => choose(key)}>
      <Icon size={15} strokeWidth={1.9} aria-hidden="true" />
    </button>)}
  </div>;
}
