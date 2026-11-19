import { t } from "./language";
import { useEffect, useRef, useState } from "react";
import { ArrowRight, Check, RotateCw, Trash2, X } from "lucide-react";
import { bytes, describeError, isRecommended, native, number, type TrashResult } from "./api";
import type { FileScan } from "./file-scan";
import { ActionProgress } from "./confirmation";

export function CleanMotion({ state }: { state: "ready" | "scanning" | "cleaning" | "done" }) {
  return <div className={`clean-motion is-${state}`} aria-hidden="true">
    <svg viewBox="0 0 108 108">
      <path className="clean-orbit" d="M54 8a46 46 0 1 1 0 92 46 46 0 1 1 0-92Z" />
      <path className="clean-track" d="M37 36h34m-38 11h42m-42 11h42m-38 11h34" />
      <path className="clean-item item-one" d="M37 36h22m4 0h8"/>
      <path className="clean-item item-two" d="M33 47h15m4 0h23"/>
      <path className="clean-item item-three" d="M33 58h29m4 0h9"/>
      <path className="clean-item item-four" d="M37 69h12m4 0h18"/>
      <path className="clean-sweep" d="M24 34h60" />
      <path className="clean-check" d="m39 53 10 10 21-22" />
    </svg>
  </div>;
}

export function QuickClean({ scan, openReview, onMoved }: { scan: FileScan; openReview: () => void; onMoved: () => void }) {
  const [open, setOpen] = useState(false);
  const [result, setResult] = useState<TrashResult>();
  const [error, setError] = useState("");
  const dialog = useRef<HTMLDialogElement>(null);
  const candidates = scan.report?.rows.filter(row => isRecommended(row.finding)) ?? [];
  const hasCandidates = candidates.length > 0;
  const state = scan.moving ? "cleaning" : scan.scanning ? "scanning" : result?.moved.length ? "done" : "ready";
  const categoryTotals = new Map<string, number>();
  for (const row of candidates) categoryTotals.set(row.category, (categoryTotals.get(row.category) ?? 0) + row.bytes);
  const subtitle = scan.moving ? t("Dateien werden in den Papierkorb verschoben …")
    : scan.scanning || !scan.started ? t("Der erste Scan läuft automatisch.")
      : scan.error && !scan.report ? t("Die Suche konnte nicht abgeschlossen werden.")
        : hasCandidates ? t("{0} empfohlene Dateien · {1}", number(candidates.length), bytes(scan.report?.recommendedBytes))
          : t("Keine empfohlenen Dateien im aktuellen Scan.");

  useEffect(() => {
    if (open) dialog.current?.showModal();
  }, [open]);

  async function move() {
    if (!hasCandidates || scan.scanning || scan.moving) return;
    setError("");
    try {
      const outcome = await scan.trash(candidates);
      setResult(outcome);
      if (outcome.moved.length) onMoved();
    } catch (error) {
      setError(describeError(error));
    }
  }

  function close() {
    if (!scan.moving) setOpen(false);
  }

  return <>
    <section className="quick-clean panel" aria-labelledby="quick-clean-title">
      <CleanMotion state={state} />
      <div className="quick-clean-body">
        <h2 id="quick-clean-title">Quick Cleanup</h2>
        <p className="quick-clean-reading">{subtitle}</p>
        <p>{t("Ältere temporäre Dateien, Protokolle und App-Caches zur Prüfung.")}</p>
        {result && <p className="quick-clean-result" role="status"><Check size={14} /> {number(result.moved.length)} {t("verschoben")}{result.failed.length ? t(" · {0} fehlgeschlagen", number(result.failed.length)) : ""}</p>}
      </div>
      <button className="button primary quick-button" disabled={!native || scan.moving} onClick={() => {
        setError(""); setResult(undefined); setOpen(true);
      }}>{t("Quick Cleanup öffnen")}<ArrowRight size={16} /></button>
    </section>
    {open && <dialog ref={dialog} className="dialog quick-dialog" aria-labelledby="quick-dialog-title"
      onCancel={event => { event.preventDefault(); close(); }}>
      <button className="icon-button quick-close" aria-label={t("Quick Cleanup schließen")} disabled={scan.moving} onClick={close}><X size={19} /></button>
      <div className="quick-dialog-heading"><CleanMotion state={state} /><div>
        <h2 id="quick-dialog-title">{scan.moving ? t("Papierkorb wird befüllt") : result ? t("Bereinigung abgeschlossen") : "Quick Cleanup"}</h2>
        <p role="status">{result ? t("{0} verschoben{1}.", number(result.moved.length), result.failed.length ? t(" · {0} fehlgeschlagen", number(result.failed.length)) : "") : subtitle}</p>
      </div></div>
      {scan.moving && <ActionProgress progress={scan.progress} />}
      {(error || result?.failed.length) && <p className="notice error" role="status">{error || result?.failed[0]}</p>}
      {scan.error && !scan.report && <p className="notice error" role="status">{scan.error}</p>}
      {!result && hasCandidates && <>
        <dl className="quick-categories">{[...categoryTotals].sort((a, b) => b[1] - a[1]).map(([category, size]) => <div key={category}><dt>{t(category)}</dt><dd>{bytes(size)}</dd></div>)}</dl>
        <p>{t("Schließe betroffene Apps vorher. Die Dateien bleiben im Papierkorb wiederherstellbar.")}</p>
      </>}
      <div className="dialog-actions quick-dialog-actions">
        {result ? <button className="button primary" onClick={close} autoFocus>{t("Fertig")}<Check size={16} /></button> : <>
          {scan.error && !scan.report ? <button className="button secondary" onClick={() => void scan.refresh()}><RotateCw size={16} /> {t("Aktualisieren")}</button>
            : <button className="button secondary" disabled={scan.moving || scan.scanning || !scan.report} onClick={() => { close(); openReview(); }}>{t("Dateien ansehen")}</button>}
          {hasCandidates && <button className="button primary" disabled={scan.moving || scan.scanning} onClick={() => void move()}>
            <Trash2 size={16} /> {scan.moving ? t("Wird verschoben …") : t("Empfohlene in den Papierkorb")}
          </button>}
        </>}
      </div>
    </dialog>}
  </>;
}
