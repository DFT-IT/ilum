import { isTauri, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useSyncExternalStore } from "react";
import english from "./messages.en.json";

type Language = "de" | "en";
const storageKey = "ilum.language";
const messages: Record<string, string> = english;
const listeners = new Set<() => void>();

function savedLanguage(): Language {
  try {
    return window.localStorage.getItem(storageKey) === "en" ? "en" : "de";
  } catch {
    return "de";
  }
}

let language: Language = savedLanguage();
document.documentElement.lang = language;

function applyLanguage(next: Language) {
  if (language === next) return;
  language = next;
  document.documentElement.lang = next;
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

export function useLanguage() {
  return useSyncExternalStore(subscribe, () => language);
}

export function currentLocale() {
  return language === "en" ? "en-GB" : "de-DE";
}

/** Translates UI copy and interpolates values, keeping filenames and paths intact. */
export function t(source: string, ...values: (string | number)[]): string {
  const key = language === "en" ? source.trim().replace(/\s+/g, " ") : "";
  const translated = Object.hasOwn(messages, key) ? messages[key] : undefined;
  const template = translated === undefined ? source
    : source.slice(0, source.length - source.trimStart().length) + translated + source.slice(source.trimEnd().length);
  return values.length ? template.replace(/\{(\d+)\}/g, (placeholder, index) => String(values[Number(index)] ?? placeholder)) : template;
}

/** Synchronizes the menu bar and dashboard without remounting either view. */
export function initializeLanguage(dashboard: boolean) {
  window.addEventListener("storage", event => {
    if (event.key === storageKey || event.key === null) applyLanguage(savedLanguage());
  });
  if (!isTauri()) return;
  void listen<Language>("language-changed", ({ payload }) => applyLanguage(payload)).catch(() => {});
  if (dashboard) void invoke("set_language", { language }).catch(() => {});
}

export function setLanguage(next: Language) {
  try { window.localStorage.setItem(storageKey, next); } catch { /* The current session can still switch. */ }
  applyLanguage(next);
  if (isTauri()) void invoke("set_language", { language: next }).catch(() => {});
}

export function localizeError(source: string): string {
  if (language === "de") return source;
  const duplicate = /^Mindestens eine bestätigte Kopie von „(.+)“ muss erhalten bleiben\.$/.exec(source);
  if (duplicate) return t("Mindestens eine bestätigte Kopie von „{0}“ muss erhalten bleiben.", duplicate[1]);
  const saveError = "Der Speicherverlauf konnte nicht gesichert werden.";
  if (source.startsWith(saveError)) return t(saveError) + source.slice(saveError.length);
  return t(source);
}
