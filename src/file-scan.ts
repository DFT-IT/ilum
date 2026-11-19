import { t } from "./language";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import { call, describeError, isRecommended, native, type FileRow, type Mode, type Report, type TrashResult } from "./api";

export type CacheScope = { scan: number; target: number; name: string; path: string };
type ScanMode = Mode | "appCache";
let nextTrashOperation = 0;
export type OperationProgress = { completed: number; total: number };
type TrashProgressEvent = OperationProgress & { operation: number };
type Snapshot = { identity: string; report: Report };

function removeMoved(report: Report, ids: number[]): Report {
  const moved = new Set(ids);
  const removed = report.rows.filter(row => moved.has(row.id));
  const rows = report.rows.filter(row => !moved.has(row.id));
  const recommended = rows.filter(row => isRecommended(row.finding));
  const sum = (files: FileRow[]) => files.reduce((total, row) => total + row.bytes, 0);
  return {
    ...report,
    rows,
    count: report.count - removed.length,
    total: report.total - sum(removed),
    oldDownloads: report.oldDownloads - sum(removed.filter(row => row.oldDownload)),
    residueCount: report.residueCount - removed.filter(row => row.appId).length,
    recommendedCount: recommended.length,
    recommendedBytes: sum(recommended),
    groups: report.groups.map(group => {
      const removedGroup = removed.filter(row => row.category === group.name);
      return { ...group, bytes: group.bytes - sum(removedGroup), count: group.count - removedGroup.length };
    }).filter(group => group.count > 0),
  };
}

export function useFileScan(mode: ScanMode, active: boolean, scope?: CacheScope) {
  const identity = scope ? `${mode}/${scope.scan}/${scope.target}` : mode;
  const scoped = mode !== "appCache" || scope != null;
  const [snapshot, setSnapshot] = useState<Snapshot>();
  const [attempted, setAttempted] = useState<string>();
  const [scanning, setScanning] = useState(false);
  const [moving, setMoving] = useState(false);
  const [progress, setProgress] = useState<OperationProgress>();
  const [failure, setFailure] = useState<{ identity: string; message: string }>();
  const running = useRef(false);
  const inventoryScan = scope?.scan;
  const inventoryTarget = scope?.target;

  const refresh = useCallback(async () => {
    if (!native || !scoped || running.current) return;
    running.current = true;
    setAttempted(identity);
    setScanning(true);
    setFailure(undefined);
    try {
      const report = await call<Report>("scan_files", { mode, inventoryScan, inventoryTarget });
      setSnapshot({ identity, report });
    } catch (error) {
      setFailure({ identity, message: describeError(error) });
    } finally {
      running.current = false;
      setScanning(false);
    }
  }, [identity, scoped, mode, inventoryScan, inventoryTarget]);

  useEffect(() => {
    if (active && attempted !== identity && !scanning && !moving) void refresh();
  }, [active, attempted, identity, moving, refresh, scanning]);

  async function trash(rows: FileRow[]): Promise<TrashResult> {
    if (running.current || rows.length === 0) throw new Error(t("Warte auf die laufende Prüfung und versuche es erneut."));
    running.current = true;
    setMoving(true);
    setProgress({ completed: 0, total: rows.length });
    const operation = ++nextTrashOperation;
    const unsubscribe = await listen<TrashProgressEvent>("trash-progress", event => {
      if (event.payload.operation === operation) setProgress(event.payload);
    }).catch(() => undefined);
    try {
      const result = await call<TrashResult>("trash_files", { mode, ids: rows.map(row => row.id), operation });
      if (result.moved.length > 0) {
        setSnapshot(previous => previous?.identity === identity ? { identity, report: removeMoved(previous.report, result.moved) } : previous);
      }
      return result;
    } finally {
      unsubscribe?.();
      running.current = false;
      setMoving(false);
      setProgress(undefined);
    }
  }

  return {
    mode, report: snapshot?.identity === identity ? snapshot.report : undefined,
    error: failure?.identity === identity ? failure.message : "",
    started: attempted === identity, scanning, moving, progress, refresh, trash,
  };
}

export type FileScan = ReturnType<typeof useFileScan>;
