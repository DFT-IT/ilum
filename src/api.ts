import { currentLocale, localizeError, t } from "./language";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";

export type Mode = "cache" | "storage" | "files";
export type Screen = "overview" | "growth" | Mode | "processes" | "dev" | "apps" | "appCache";
export type Process = { pid: number; name: string; memory: number; cpu: number; started: number; canTerminate: boolean };
export type Snapshot = {
  cpu: number;
  cores: number;
  ramUsed: number;
  ramTotal: number;
  diskUsed: number | null;
  diskTotal: number | null;
  uptime: number;
  computerName: string | null;
  memoryPressure: "normal" | "warning" | "critical" | null;
  processes: Process[];
};
export type Finding = "standard" | "oldTemp" | "oldLog" | "oldCache" | "residualCache" | "residualData";
export const isRecommended = (finding: Finding) => finding === "oldTemp" || finding === "oldLog" || finding === "oldCache";

export type FileRow = {
  id: number; name: string; path: string; bytes: number; hardLinks: number; modifiedAt: number | null; category: string; oldDownload: boolean; duplicate: string | null; finding: Finding; appId: string | null;
};
export type Group = { name: string; bytes: number; count: number };
export type Report = { rows: FileRow[]; groups: Group[]; total: number; count: number; skipped: number; truncated: boolean; oldDownloads: number; recommendedBytes: number; recommendedCount: number;
  residueCount: number; inventoryAvailable: boolean;
};
export type TrashResult = { moved: number[]; failed: string[] };

export const native = isTauri();
export function call<T>(name: string, args?: Record<string, unknown>) {
  return native ? invoke<T>(name, args) : Promise.reject(t("Diese Funktion ist in der macOS-App verfügbar."));
}
export const describeError = (error: unknown) => typeof error === "string" ? localizeError(error) : error instanceof Error ? localizeError(error.message) : t("Die Aktion ist fehlgeschlagen.");
export const number = (value: number, digits = 0) => value.toLocaleString(currentLocale(), { maximumFractionDigits: digits });
export const bytes = (value: number | null | undefined) => {
  if (value == null) return "—";
  if (!value) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const unit = Math.min(units.length - 1, Math.max(0, Math.floor(Math.log(value) / Math.log(1000))));
  return `${number(value / 1000 ** unit, unit === 0 ? 0 : 1)} ${units[unit]}`;
};
const dateFormats = {
  "de-DE": new Intl.DateTimeFormat("de-DE", { dateStyle: "short", timeStyle: "short" }),
  "en-GB": new Intl.DateTimeFormat("en-GB", { dateStyle: "short", timeStyle: "short" }),
};
export function dateTime(timestamp: number | null | undefined) {
  return timestamp == null ? "—" : dateFormats[currentLocale()].format(timestamp * 1000);
}

export function sizeValue(value: string) {
  const parts = value.trim().match(/^([\d.]+)\s*(B|kB|KB|MB|GB|TB|PB)$/i);
  const scale = parts ? ["B", "KB", "MB", "GB", "TB", "PB"].indexOf(parts[2].toUpperCase()) : -1;
  return parts && scale >= 0 ? Number(parts[1]) * 1000 ** scale : null;
}

export const ratio = (used: number | null | undefined, total: number | null | undefined) =>
  used != null && total ? Math.min(100, 100 * used / total) : null;
export const percent = (value: number | null | undefined) => value == null ? "—" : `${number(value)} %`;
export const uptime = (seconds: number) => {
  const hours = Math.floor(seconds / 3600);
  const days = Math.floor(hours / 24);
  return days ? t("{0} T {1} Std.", days, hours % 24) : hours ? t("{0} Std. {1} Min.", hours, Math.floor(seconds / 60) % 60) : t("{0} Min.", Math.floor(seconds / 60));
};

export function useTelemetry() {
  const [sample, setSample] = useState<Snapshot>();
  const [history, setHistory] = useState<number[]>([]);
  const [error, setError] = useState("");
  const [updated, setUpdated] = useState<Date>();
  const [revision, setRevision] = useState(0);
  const refresh = useCallback(() => setRevision(value => value + 1), []);

  useEffect(() => {
    if (!native) return;
    let alive = true;
    let pending = false;
    let timer: ReturnType<typeof setTimeout>;
    const pull = async () => {
      if (!alive || pending) return;
      clearTimeout(timer);
      pending = true;
      let delay = 3300;
      try {
        const result = await call<Snapshot | null>("telemetry");
        if (alive && result) {
          setSample(result);
          setHistory(values => [...values.slice(-35), result.cpu]);
          setUpdated(new Date());
          setError("");
          delay = 2000;
        }
      } catch (reason) {
        if (alive) setError(describeError(reason));
      } finally {
        pending = false;
        if (alive) timer = setTimeout(pull, delay);
      }
    };
    const listener = listen("surface-shown", pull);
    void pull();
    return () => {
      alive = false;
      clearTimeout(timer);
      void listener.then(dispose => dispose()).catch(() => {});
    };
  }, [revision]);

  return { sample, history, updated, error, refresh };
}
