import { t } from "./language";
import { useEffect, useId, useRef, type ReactNode } from "react";
import { CircleAlert, LoaderCircle } from "lucide-react";
import { number } from "./api";
import type { OperationProgress } from "./file-scan";

export function ActionProgress({ progress }: { progress?: OperationProgress }) {
  return <div className="action-progress" role="status">
    <progress aria-label={t("Dateien verarbeitet")} max={progress?.total ?? 1} value={progress?.completed} />
    {progress && <small>{number(progress.completed)} {t("von ")}{number(progress.total)} {t("verarbeitet")}</small>}
  </div>;
}


export function Confirmation({ title, detail, action, busy, progress, onClose, onConfirm }: {
  title: string; detail: ReactNode; action: string; busy: boolean; progress?: OperationProgress; onClose: () => void; onConfirm: () => void;
}) {
  const titleId = useId();
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => { dialog.current?.showModal(); }, []);
  return <dialog ref={dialog} className="dialog" aria-labelledby={titleId} onCancel={event => { event.preventDefault(); if (!busy) onClose(); }}>
    <div className="dialog-icon"><CircleAlert size={21} /></div>
    <h2 id={titleId}>{title}</h2>
    <p>{detail}</p>
    {busy && <ActionProgress progress={progress} />}
    <div className="dialog-actions">
      <button className="button secondary" onClick={onClose} autoFocus disabled={busy}>{t("Abbrechen")}</button>
      <button className="button primary" onClick={onConfirm} disabled={busy}>{busy && <LoaderCircle className="spin" size={16} />}{action}</button>
    </div>
  </dialog>;
}
